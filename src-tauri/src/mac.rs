//! The two things the webview cannot do itself: the macOS share sheet, and a
//! native print of an image file. Both are AppKit, both must run on the main
//! thread, and both take the finished overlay PNG - the exact bytes that go to
//! paper - rather than anything the webview has rendered.
//!
//! Why not `Webview::print()`: it exists, and on macOS Tauri even patches
//! `window.print()` to route through it. But it prints the DOM by way of
//! WKWebView's print operation, so the photo is rasterised by the web engine at
//! whatever the print context offers. For a codebase whose whole purpose is not
//! resampling the photo, that is the wrong pipeline. `NSPrintOperation` over an
//! `NSImageView` hands the PNG to the print system as-is and gives us the paper
//! size directly.
//!
//! Why not a Swift helper: it would be a second signed binary on the notarization
//! surface, and a separate process cannot anchor a share popover to our own view.

use std::cell::RefCell;
use std::ffi::c_void;
use std::path::Path;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
// alloc() lives on these traits: AnyThread for classes usable anywhere (NSImage),
// MainThreadOnly for views and the picker, whose alloc takes the marker.
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSImage, NSImageScaling, NSImageView, NSPaperOrientation, NSPrintInfo, NSPrintOperation,
    NSPrintPanelOptions, NSPrinter, NSPrintingPaginationMode, NSSharingServicePicker, NSView,
    NSWindow,
};
use objc2_foundation::{NSArray, NSPoint, NSRect, NSRectEdge, NSSize, NSString, NSURL};
use tauri::{Manager, WebviewWindow};

use crate::problem::{detail, Level, Notify, Payload, Problem};

/// PostScript points per millimetre (72 per inch).
const PT_PER_MM: f64 = 72.0 / 25.4;

thread_local! {
    // AppKit does not retain the picker on our behalf, and the popover is torn
    // down the moment it deallocates - which, without this, is the end of the
    // closure that showed it. Keeping the most recent one alive bounds that to a
    // single object rather than leaking one per share.
    static PICKER: RefCell<Option<Retained<NSSharingServicePicker>>> = const { RefCell::new(None) };
}

/// Where on screen the Share button is, in CSS pixels relative to the webview.
///
/// Exported to TypeScript like everything else that crosses the boundary. It had
/// no TS type at all before: the frontend built an anonymous object literal, so a
/// renamed field here would have broken the share sheet silently at runtime.
#[derive(Debug, Clone, Copy, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Anchor {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Shows the share sheet for `path`, anchored to the button that asked for it.
pub fn share(window: &WebviewWindow, path: &Path, anchor: Anchor) -> Result<(), Payload> {
    let path = path.to_path_buf();
    // The closure below runs after this function has already returned, so a
    // problem it discovers cannot travel in the Err - it goes out as a notice.
    let app = window.app_handle().clone();
    window
        .with_webview(move |webview| {
            let Some(mtm) = MainThreadMarker::new() else {
                Problem::WindowNotReady {
                    what: "share sheet",
                    detail: "with_webview did not run on the main thread".into(),
                }
                .notify(&app, Level::Error);
                return;
            };

            // On macOS the inner webview is a WKWebView, which is an NSView. That is
            // the thing to anchor to - not ns_window() - because the popover's rect
            // is in the view's coordinate space.
            let view: &NSView = unsafe { &*(webview.inner() as *const NSView) };

            let url = NSURL::fileURLWithPath(&NSString::from_str(&path.display().to_string()));
            let items: Retained<NSArray<AnyObject>> =
                NSArray::from_retained_slice(&[Retained::into_super(Retained::into_super(url))]);

            // CSS coordinates have their origin at the top-left; AppKit's default is
            // bottom-left. WKWebView happens to be flipped, but reading the flag beats
            // remembering that.
            let bounds = view.bounds();
            let rect = if view.isFlipped() {
                NSRect::new(NSPoint::new(anchor.x, anchor.y), NSSize::new(anchor.width, anchor.height))
            } else {
                NSRect::new(
                    NSPoint::new(anchor.x, bounds.size.height - anchor.y - anchor.height),
                    NSSize::new(anchor.width, anchor.height),
                )
            };
            // "Below the button" is the far edge along y in a flipped view.
            let edge = if view.isFlipped() { NSRectEdge::NSMaxYEdge } else { NSRectEdge::NSMinYEdge };

            let _ = mtm;
            let picker = unsafe { NSSharingServicePicker::initWithItems(NSSharingServicePicker::alloc(), &items) };
            picker.showRelativeToRect_ofView_preferredEdge(rect, view, edge);
            PICKER.with(|slot| slot.borrow_mut().replace(picker));
        })
        .map_err(|e| Problem::WindowNotReady { what: "share sheet", detail: detail(e) }.say())
}

/// Opens the print dialog for `path`, set up for the photo paper: the driver's
/// named paper `paper` (borderless, on the SELPHY), oriented to match the photo,
/// no margins, image scaled to fit the sheet.
///
/// Selecting the paper BY NAME is the whole point, and it was learned the hard
/// way. The SELPHY driver defines two papers with identical dimensions:
///
/// ```text
/// *PageSize Postcard            ImageableArea 7.09 10.49 276.38 409.04   (margins)
/// *PageSize Postcard.Fullbleed  ImageableArea 0    0     283.46 419.53   (edge to edge)
/// ```
///
/// macOS shows the second as "Postcard Borderless". Asking for a custom size of
/// 283.46 x 419.53 pt resolves to the first, because that is the driver's default,
/// and the print comes back with a white border. The size is only a fallback for
/// a printer that has no paper of that name at all.
///
/// `title` names the job. Without one AppKit calls it "Untitled", because the
/// operation is over a bare view rather than an `NSDocument` - so the queue and
/// the panel's Save as PDF both offer that instead of the photo's own name.
///
/// `setup` says which printer and which paper. Both used to be fixed - the
/// system default printer and a hard-coded 100 x 148 mm - which was right for
/// exactly one printer. They come from the settings window now, and the expected
/// size is the one the crop was made for, so "did the driver accept the paper"
/// is checked against what the photo actually is.
pub fn print(
    window: &WebviewWindow,
    path: &Path,
    landscape: bool,
    setup: &PrintSetup,
    title: &str,
) -> Result<(), Payload> {
    let path = path.to_path_buf();
    let PrintSetup { printer, paper, short_mm, long_mm } = setup.clone();
    let (short_pt, long_pt) = (short_mm * PT_PER_MM, long_mm * PT_PER_MM);
    let title = title.to_string();
    // Everything below the dispatch runs after this function has returned Ok, so
    // it cannot report through the Err. Two things in there are worth telling the
    // operator - an unreadable PNG and a paper the printer does not have - and
    // both go out as notices instead.
    let app = window.app_handle().clone();
    let ns_window = window
        .ns_window()
        .map_err(|e| Problem::WindowNotReady { what: "print dialog", detail: detail(e) }.say())? as usize;

    window
        .run_on_main_thread(move || {
            let Some(mtm) = MainThreadMarker::new() else {
                Problem::WindowNotReady {
                    what: "print dialog",
                    detail: "run_on_main_thread did not run on the main thread".into(),
                }
                .notify(&app, Level::Error);
                return;
            };
            let ns_window: &NSWindow = unsafe { &*(ns_window as *const NSWindow) };

            let Some(image) = NSImage::initWithContentsOfFile(NSImage::alloc(), &NSString::from_str(&path.display().to_string())) else {
                Problem::PrintUnreadable { path: path.clone() }.notify(&app, Level::Error);
                return;
            };

            let orientation = if landscape { NSPaperOrientation::Landscape } else { NSPaperOrientation::Portrait };

            let info = NSPrintInfo::new();

            // The printer before the paper: paper names are the printer's own, so
            // setting the paper on the default printer and then switching would
            // check the name against the wrong driver. An empty name is the system
            // default, which is what `NSPrintInfo::new` already has.
            if !printer.is_empty() {
                match NSPrinter::printerWithName(&NSString::from_str(&printer)) {
                    Some(found) => info.setPrinter(&found),
                    None => {
                        Problem::PrinterMissing { name: printer.clone() }.notify(&app, Level::Warning);
                        log::warn!("print: no printer called {printer}, using the default");
                    }
                }
            }

            // paperName takes the PPD keyword ("Postcard.Fullbleed"), not the label
            // the dialog shows. Set it first and then check whether the driver
            // accepted it: on a printer that has no such paper the size stays at
            // whatever the default was, and the fallback below sets it by hand.
            if !paper.is_empty() {
                info.setPaperName(Some(&NSString::from_str(&paper)));
            }
            let size = info.paperSize();
            let accepted = !paper.is_empty()
                && (size.width - short_pt).abs() < 1.0
                && (size.height - long_pt).abs() < 1.0;
            if accepted {
                log::info!("print: using paper {paper} ({:.1} x {:.1} pt)", size.width, size.height);
            } else {
                if !paper.is_empty() {
                    // The operator can act on this right now: the panel below is
                    // already forced to show Paper Size. It deliberately does not
                    // name `paper` - that is a PPD keyword and pure jargon.
                    Problem::PaperFallback.notify(&app, Level::Warning);
                    log::warn!(
                        "print: no paper called {paper} on this printer (got {:.1} x {:.1} pt), \
                         falling back to a custom {short_mm} x {long_mm} mm",
                        size.width, size.height
                    );
                }
                info.setPaperSize(NSSize::new(short_pt, long_pt));
            }

            // Orientation flips the named paper for a landscape photo. Margins are
            // zeroed because the overlay already is the full print - the branding
            // runs to the paper's edge by design.
            info.setOrientation(orientation);
            let (width, height) = if landscape { (long_pt, short_pt) } else { (short_pt, long_pt) };
            info.setLeftMargin(0.0);
            info.setRightMargin(0.0);
            info.setTopMargin(0.0);
            info.setBottomMargin(0.0);
            info.setHorizontalPagination(NSPrintingPaginationMode::Fit);
            info.setVerticalPagination(NSPrintingPaginationMode::Fit);
            info.setHorizontallyCentered(true);
            info.setVerticallyCentered(true);

            // The view is exactly one sheet, and the image scales proportionally
            // into it. The crop already gave the photo the paper's shape, so the
            // only scaling here is the rounding residue.
            let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, height));
            let view = NSImageView::initWithFrame(NSImageView::alloc(mtm), frame);
            view.setImage(Some(&image));
            view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);

            let operation = NSPrintOperation::printOperationWithView_printInfo(&view, &info);
            if !title.is_empty() {
                operation.setJobTitle(Some(&NSString::from_str(&title)));
            }

            // The default panel hides Paper Size and Orientation, which is exactly
            // what the operator needs to see to confirm "Postcard Borderless" is
            // selected. Preview is what shows the borderless result before paper
            // is spent.
            let panel = operation.printPanel();
            panel.setOptions(
                NSPrintPanelOptions::ShowsCopies
                    | NSPrintPanelOptions::ShowsPageRange
                    | NSPrintPanelOptions::ShowsPaperSize
                    | NSPrintPanelOptions::ShowsOrientation
                    | NSPrintPanelOptions::ShowsScaling
                    | NSPrintPanelOptions::ShowsPreview,
            );

            unsafe {
                operation.setShowsPrintPanel(true);
                operation.setShowsProgressPanel(true);
                // As a sheet on our window, so it cannot be lost behind the app.
                operation.runOperationModalForWindow_delegate_didRunSelector_contextInfo(
                    ns_window,
                    None,
                    None,
                    std::ptr::null_mut(),
                );
            }
        })
        .map_err(|e| Problem::WindowNotReady { what: "print dialog", detail: detail(e) }.say())
}

/// Which printer, which paper, and the size the crop was made for.
#[derive(Debug, Clone, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct PrintSetup {
    /// The printer's display name, as `NSPrinter::printerWithName` takes it.
    /// Empty means the system default.
    pub printer: String,
    /// The driver's paper keyword. Empty means let the driver pick.
    pub paper: String,
    pub short_mm: f64,
    pub long_mm: f64,
}

/// One printer, as the settings window lists it.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct Printer {
    pub name: String,
    pub is_default: bool,
}

/// One paper a printer offers.
///
/// `name` is the driver's keyword and what gets stored; `label` is what macOS
/// shows for it and what the operator reads. The two differ in exactly the way
/// that bit the SELPHY: `Postcard.Fullbleed` is shown as "Postcard Borderless".
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct Paper {
    pub name: String,
    pub label: String,
    pub short_mm: f64,
    pub long_mm: f64,
    /// No unprintable margin on any side.
    ///
    /// **From the margins, never from the name.** Only Canon's drivers spell
    /// borderless as `.Fullbleed`; HP, Epson and driverless AirPrint queues use
    /// other names or none, and every one of them reports its margins.
    pub borderless: bool,
}

// PrintCore, by hand. objc2 has no binding for it, and AppKit's NSPrinter can
// answer "how big is the paper called X" but cannot list the papers a printer
// has - which is the one question the settings window needs answered. A
// handful of C calls in ApplicationServices is the whole of it.
//
// Ownership follows Core Foundation's naming rule: `Create` hands back
// something to `CFRelease`, `Get` does not.
type CFTypeRef = *const c_void;

/// `PMPaperMargins`, which PrintCore declares as a `PMRect`: top, left, bottom,
/// right, in points.
#[repr(C)]
#[derive(Default)]
struct PMRect {
    top: f64,
    left: f64,
    bottom: f64,
    right: f64,
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn PMServerCreatePrinterList(server: CFTypeRef, list: *mut CFTypeRef) -> i32;
    fn PMPrinterGetName(printer: CFTypeRef) -> CFTypeRef;
    fn PMPrinterIsDefault(printer: CFTypeRef) -> u8;
    fn PMPrinterGetPaperList(printer: CFTypeRef, list: *mut CFTypeRef) -> i32;
    fn PMPaperGetPPDPaperName(paper: CFTypeRef, name: *mut CFTypeRef) -> i32;
    fn PMPaperCreateLocalizedName(paper: CFTypeRef, printer: CFTypeRef, name: *mut CFTypeRef) -> i32;
    fn PMPaperGetWidth(paper: CFTypeRef, width: *mut f64) -> i32;
    fn PMPaperGetHeight(paper: CFTypeRef, height: *mut f64) -> i32;
    fn PMPaperGetMargins(paper: CFTypeRef, margins: *mut PMRect) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFArrayGetCount(array: CFTypeRef) -> isize;
    fn CFArrayGetValueAtIndex(array: CFTypeRef, index: isize) -> CFTypeRef;
    fn CFRelease(cf: CFTypeRef);
}

/// A `CFStringRef` read as a Rust string. CFString and NSString are toll-free
/// bridged, so this borrows rather than copies until `to_string`.
unsafe fn cf_string(cf: CFTypeRef) -> Option<String> {
    (!cf.is_null()).then(|| (*(cf as *const NSString)).to_string())
}

/// The elements of a `CFArrayRef`, borrowed.
unsafe fn cf_items(array: CFTypeRef) -> Vec<CFTypeRef> {
    (0..CFArrayGetCount(array)).map(|i| CFArrayGetValueAtIndex(array, i)).collect()
}

/// Runs `f` over the local printer list, releasing it afterwards.
fn with_printers<T>(f: impl FnOnce(&[CFTypeRef]) -> T) -> T {
    let mut list: CFTypeRef = std::ptr::null();
    // A null server is `kPMServerLocal`, which is every queue this Mac knows -
    // including the network ones, which is why the commands calling this are
    // async: a sleeping office printer can take a moment to answer.
    let status = unsafe { PMServerCreatePrinterList(std::ptr::null(), &mut list) };
    if status != 0 || list.is_null() {
        log::warn!("print: cannot list printers (PrintCore status {status})");
        return f(&[]);
    }
    let items = unsafe { cf_items(list) };
    let result = f(&items);
    unsafe { CFRelease(list) };
    result
}

/// Every printer this Mac knows, the default first.
pub fn printers() -> Vec<Printer> {
    let mut printers = with_printers(|items| {
        items
            .iter()
            .filter_map(|&printer| {
                let name = unsafe { cf_string(PMPrinterGetName(printer)) }?;
                let is_default = unsafe { PMPrinterIsDefault(printer) } != 0;
                Some(Printer { name, is_default })
            })
            .collect::<Vec<_>>()
    });
    printers.sort_by(|a, b| b.is_default.cmp(&a.is_default).then_with(|| a.name.cmp(&b.name)));
    printers
}

/// The papers `printer` offers, or the default printer's when it is empty.
///
/// Sorted for a photo kiosk rather than alphabetically: borderless first, then
/// anything photo-sized, then the rest - so the paper an operator is after is at
/// the top and A4 and the envelopes are at the bottom. An unknown printer gives an
/// empty list; the window shows the saved paper on its own in that case.
pub fn papers(printer: &str) -> Vec<Paper> {
    let mut papers = with_printers(|items| {
        let Some(&found) = items.iter().find(|&&candidate| {
            if printer.is_empty() {
                unsafe { PMPrinterIsDefault(candidate) != 0 }
            } else {
                unsafe { cf_string(PMPrinterGetName(candidate)) }.as_deref() == Some(printer)
            }
        }) else {
            return Vec::new();
        };

        let mut list: CFTypeRef = std::ptr::null();
        if unsafe { PMPrinterGetPaperList(found, &mut list) } != 0 || list.is_null() {
            return Vec::new();
        }
        unsafe { cf_items(list) }
            .into_iter()
            .filter_map(|paper| unsafe { read_paper(paper, found) })
            .collect()
    });

    // A driver can list the same keyword more than once, once per tray.
    let mut seen = std::collections::HashSet::new();
    papers.retain(|paper| seen.insert(paper.name.clone()));
    let photo_sized = |paper: &Paper| paper.short_mm <= 130.0;
    papers.sort_by(|a, b| {
        (b.borderless, photo_sized(b))
            .cmp(&(a.borderless, photo_sized(a)))
            .then_with(|| (a.short_mm * a.long_mm).total_cmp(&(b.short_mm * b.long_mm)))
    });
    papers
}

/// # Safety
/// `paper` and `printer` must be live PrintCore objects from the same list.
unsafe fn read_paper(paper: CFTypeRef, printer: CFTypeRef) -> Option<Paper> {
    let mut keyword: CFTypeRef = std::ptr::null();
    if PMPaperGetPPDPaperName(paper, &mut keyword) != 0 {
        return None;
    }
    let name = cf_string(keyword)?;

    let mut localized: CFTypeRef = std::ptr::null();
    let label = if PMPaperCreateLocalizedName(paper, printer, &mut localized) == 0 && !localized.is_null() {
        let label = cf_string(localized);
        CFRelease(localized);
        label
    } else {
        None
    }
    .unwrap_or_else(|| name.clone());

    let (mut width, mut height) = (0.0, 0.0);
    if PMPaperGetWidth(paper, &mut width) != 0 || PMPaperGetHeight(paper, &mut height) != 0 {
        return None;
    }
    let mut margins = PMRect::default();
    let borderless = PMPaperGetMargins(paper, &mut margins) == 0
        && [margins.top, margins.left, margins.bottom, margins.right]
            .iter()
            .all(|margin| margin.abs() < 0.5);

    // Tenths of a millimetre: the crop is only as exact as the ratio, and 4x6"
    // reading back as 101.6 x 152.4 is what an operator expects to see.
    let mm = |pt: f64| (pt / PT_PER_MM * 10.0).round() / 10.0;
    let (a, b) = (mm(width), mm(height));
    Some(Paper { name, label, short_mm: a.min(b), long_mm: a.max(b), borderless })
}

/// Against the printers actually installed on this Mac, so `#[ignore]`d: the
/// answer depends on the machine, and CI has none. Run it with
/// `cargo test -p snappr -- --ignored --nocapture` and read what it prints - it
/// is the only check that the hand-written PrintCore calls above read real
/// drivers correctly.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn lists_this_macs_printers_and_their_papers() {
        let printers = printers();
        assert!(!printers.is_empty(), "this Mac has no printers to read");
        for printer in &printers {
            let papers = papers(&printer.name);
            println!("{} (default: {}) - {} papers", printer.name, printer.is_default, papers.len());
            for paper in papers.iter().take(6) {
                println!(
                    "    {:<28} {:<36} {:>6.1} x {:>6.1} mm{}",
                    paper.name,
                    paper.label,
                    paper.short_mm,
                    paper.long_mm,
                    if paper.borderless { "  borderless" } else { "" }
                );
            }
        }
    }
}
