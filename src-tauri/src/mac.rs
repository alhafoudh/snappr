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
use std::path::Path;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
// alloc() lives on these traits: AnyThread for classes usable anywhere (NSImage),
// MainThreadOnly for views and the picker, whose alloc takes the marker.
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSImage, NSImageScaling, NSImageView, NSPaperOrientation, NSPrintInfo, NSPrintOperation,
    NSPrintPanelOptions, NSPrintingPaginationMode, NSSharingServicePicker, NSView, NSWindow,
};
use objc2_foundation::{NSArray, NSPoint, NSRect, NSRectEdge, NSSize, NSString, NSURL};
use tauri::{Manager, WebviewWindow};

use crate::problem::{detail, Level, Notify, Payload, Problem};

/// 100 x 148 mm in PostScript points (72 per inch).
const SHORT_PT: f64 = 100.0 / 25.4 * 72.0;
const LONG_PT: f64 = 148.0 / 25.4 * 72.0;

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
pub fn print(
    window: &WebviewWindow,
    path: &Path,
    landscape: bool,
    paper: &str,
    title: &str,
) -> Result<(), Payload> {
    let path = path.to_path_buf();
    let paper = paper.to_string();
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

            // paperName takes the PPD keyword ("Postcard.Fullbleed"), not the label
            // the dialog shows. Set it first and then check whether the driver
            // accepted it: on a printer that has no such paper the size stays at
            // whatever the default was, and the fallback below sets it by hand.
            if !paper.is_empty() {
                info.setPaperName(Some(&NSString::from_str(&paper)));
            }
            let size = info.paperSize();
            let accepted = !paper.is_empty()
                && (size.width - SHORT_PT).abs() < 1.0
                && (size.height - LONG_PT).abs() < 1.0;
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
                         falling back to a custom 100 x 148 mm",
                        size.width, size.height
                    );
                }
                info.setPaperSize(NSSize::new(SHORT_PT, LONG_PT));
            }

            // Orientation flips the named paper for a landscape photo. Margins are
            // zeroed because the overlay already is the full print - the branding
            // runs to the paper's edge by design.
            info.setOrientation(orientation);
            let (width, height) = if landscape { (LONG_PT, SHORT_PT) } else { (SHORT_PT, LONG_PT) };
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
