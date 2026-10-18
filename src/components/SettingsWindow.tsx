import { useEffect, useMemo, useState, type ReactNode } from "react";
import {
  Box,
  Button,
  Callout,
  CheckboxCards,
  Flex,
  Grid,
  ScrollArea,
  Select,
  Separator,
  Switch,
  Tabs,
  Text,
  TextField,
} from "@radix-ui/themes";
import { ExclamationTriangleIcon, InfoCircledIcon } from "@radix-ui/react-icons";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { cmd, isProblem, sentence, type Problem } from "../ipc";
import type { Paper, Printer, Settings } from "../types";

/**
 * The settings window: every setting the app has that is not an overlay.
 *
 * **A separate window, not a dialog in the main one.** "Modals" in
 * docs/desktop-app.md allows a modal for exactly two things and settings are
 * neither, and at an event the filmstrip has to keep showing arrivals while
 * someone is changing the paper. Rust builds it on demand (Snappr > Settings...,
 * Cmd+,) and `main.tsx` renders this instead of `App` from its label.
 *
 * **A draft with Save and Cancel**, where the main window's controls save on the
 * click. Typing "1" on the way to "148" mm must not save a 1 mm paper, and the
 * save makes the whole strip again - not something to do per keystroke. Closing
 * the window, Escape included, is Cancel: a discard prompt would be a third kind
 * of modal, and nothing is lost that is not still on screen in the fields.
 *
 * **Validation is Rust's.** `save_settings` runs the same `validate` the CLI
 * runs on its TOML, and a refusal comes back with a catalogue code; `FIELD_FOR`
 * turns that code into the field to mark. The only check made here is that a
 * number field holds a number, and that one is not a message - a field that does
 * not parse just goes back to its last value when it loses focus.
 *
 * Overlays are not here. The panel down the left of the main window owns them,
 * and two places to switch one on is how the two end up disagreeing.
 */

type Field =
  | "watch_dir"
  | "extensions"
  | "paper"
  | "background"
  | "output_suffix"
  | "png_compression"
  | "preview"
  | "stability";

type TabName = "general" | "print" | "advanced";

/**
 * Where each refusal belongs. Keyed on the catalogue code, which is stable
 * across rewordings - the one property that makes branching on it safe. A code
 * that is not here goes to the footer.
 */
const FIELD_FOR: Record<string, Field> = {
  not_a_folder: "watch_dir",
  cannot_watch: "watch_dir",
  no_extensions: "extensions",
  png_watched: "extensions",
  paper_size_invalid: "paper",
  background_not_a_colour: "background",
  output_suffix_invalid: "output_suffix",
  compression_out_of_range: "png_compression",
  preview_size_invalid: "preview",
  preview_quality_out_of_range: "preview",
  stability_invalid: "stability",
};

const TAB_FOR: Record<Field, TabName> = {
  watch_dir: "general",
  extensions: "general",
  paper: "print",
  background: "print",
  output_suffix: "advanced",
  png_compression: "advanced",
  preview: "advanced",
  stability: "advanced",
};

/**
 * The kinds of photo snappr can read. A chip per kind rather than a text field:
 * anything else is either a format libvips will not load or `png`, which the
 * self-output guard refuses. A hand-edited `settings.json` holding something
 * else still shows it, so it can be switched off.
 */
const KNOWN_EXTENSIONS = ["heic", "heif", "jpg", "jpeg"];

/** Radix's Select refuses an empty value, so the two "nothing chosen" states get one. */
const DEFAULT_PRINTER = "__default__";
const DRIVER_PAPER = "__driver__";

/**
 * What `save_settings` takes, as one comparable value. The overlay slots, the
 * library and the watch toggle are not in it, because the window does not edit
 * them - so a toggle clicked in the main window while this is open does not make
 * the draft look unsaved.
 */
function editable(s: Settings) {
  return JSON.stringify([
    s.watch_dir,
    s.extensions,
    s.output_suffix,
    s.background,
    s.png_compression,
    s.print_short_mm,
    s.print_long_mm,
    s.preview_max_px,
    s.preview_thumb_px,
    s.preview_quality,
    s.stability_checks,
    s.stability_timeout_secs,
    s.printer,
    s.print_paper_name,
  ]);
}

/** Mirrors `Settings::prints_changed` in settings.rs, to say so before Save. */
function printsChange(a: Settings, b: Settings) {
  return (
    a.background !== b.background ||
    a.print_short_mm !== b.print_short_mm ||
    a.print_long_mm !== b.print_long_mm ||
    a.preview_max_px !== b.preview_max_px ||
    a.preview_thumb_px !== b.preview_thumb_px ||
    a.preview_quality !== b.preview_quality
  );
}

/** What each tab's Reset puts back. */
function resetTab(tab: TabName, draft: Settings, defaults: Settings): Settings {
  switch (tab) {
    case "general":
      return { ...draft, watch_dir: defaults.watch_dir, extensions: defaults.extensions };
    case "print":
      return {
        ...draft,
        printer: defaults.printer,
        print_paper_name: defaults.print_paper_name,
        print_short_mm: defaults.print_short_mm,
        print_long_mm: defaults.print_long_mm,
        background: defaults.background,
      };
    case "advanced":
      return {
        ...draft,
        output_suffix: defaults.output_suffix,
        png_compression: defaults.png_compression,
        preview_max_px: defaults.preview_max_px,
        preview_thumb_px: defaults.preview_thumb_px,
        preview_quality: defaults.preview_quality,
        stability_checks: defaults.stability_checks,
        stability_timeout_secs: defaults.stability_timeout_secs,
      };
  }
}

const mm = (value: number) => `${Number(value.toFixed(1))}`;

/**
 * The colour well. Radix has no colour input, so this is the browser's own, sized
 * to sit beside a size-2 text field.
 */
const swatch = {
  width: "var(--space-7)",
  height: "var(--space-6)",
  padding: 0,
  border: "none",
  background: "none",
  cursor: "pointer",
} as const;

export function SettingsWindow() {
  const [saved, setSaved] = useState<Settings | null>(null);
  const [draft, setDraft] = useState<Settings | null>(null);
  const [defaults, setDefaults] = useState<Settings | null>(null);
  const [printers, setPrinters] = useState<Printer[] | null>(null);
  const [papers, setPapers] = useState<Paper[] | null>(null);
  const [customSize, setCustomSize] = useState(false);
  const [shapes, setShapes] = useState<string[]>([]);
  const [strip, setStrip] = useState(0);
  const [selected, setSelected] = useState<string | null>(null);
  const [tab, setTab] = useState<TabName>("general");
  const [problem, setProblem] = useState<Problem | null>(null);
  const [footerText, setFooterText] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const close = () => void getCurrentWindow().close();

  useEffect(() => {
    void (async () => {
      const [current, fresh, photos, remembered] = await Promise.all([
        cmd.getSettings(),
        cmd.defaultSettings(),
        cmd.listPhotos(),
        cmd.selectedPhoto(),
      ]);
      setSaved(current);
      setDraft(current);
      setDefaults(fresh);
      setStrip(photos.length);
      setSelected(remembered);
      // After the form is up: a network printer that is asleep can take a moment.
      setPrinters(await cmd.listPrinters());
    })();
  }, []);

  // Escape is Cancel. Cmd+W already closes the window through the app menu.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") close();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const printer = draft?.printer;
  useEffect(() => {
    if (printer === undefined) return;
    let current = true;
    setPapers(null);
    void cmd.listPapers(printer).then((list) => current && setPapers(list));
    return () => {
      current = false;
    };
  }, [printer]);

  // The saved paper decides whether the size starts out locked: a size that
  // matches the chosen paper is the paper's, anything else was typed.
  const paperName = saved?.print_paper_name;
  useEffect(() => {
    if (!papers || !saved) return;
    const paper = papers.find((p) => p.name === paperName);
    setCustomSize(
      !paper ||
        paper.short_mm !== saved.print_short_mm ||
        paper.long_mm !== saved.print_long_mm,
    );
  }, [papers, paperName, saved]);

  const shortMm = draft?.print_short_mm;
  const longMm = draft?.print_long_mm;
  useEffect(() => {
    if (shortMm === undefined || longMm === undefined) return;
    let current = true;
    const timer = setTimeout(() => {
      void cmd.checkOverlayShapes(shortMm, longMm).then((found) => current && setShapes(found));
    }, 250);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [shortMm, longMm]);

  const dirty = useMemo(
    () => !!draft && !!saved && editable(draft) !== editable(saved),
    [draft, saved],
  );

  if (!draft || !saved || !defaults) {
    return null;
  }

  const update = (patch: Partial<Settings>) => {
    setDraft({ ...draft, ...patch });
    setProblem(null);
    setFooterText(null);
  };

  const fail = (error: unknown) => {
    if (isProblem(error)) {
      setProblem(error);
      const field = FIELD_FOR[error.code];
      if (field) setTab(TAB_FOR[field]);
    } else {
      setProblem({ code: "", message: sentence(error) });
    }
  };

  const errorFor = (field: Field) =>
    problem && FIELD_FOR[problem.code] === field ? problem.message : null;
  const footerProblem = problem && !FIELD_FOR[problem.code] ? problem.message : null;

  const save = async () => {
    setSaving(true);
    try {
      await cmd.saveSettings(draft);
      close();
    } catch (error) {
      fail(error);
    } finally {
      setSaving(false);
    }
  };

  const chooseFolder = async () => {
    const dir = await cmd.pickWatchDir(draft.watch_dir);
    if (dir) update({ watch_dir: dir });
  };

  const choosePaper = (name: string) => {
    if (name === DRIVER_PAPER) {
      update({ print_paper_name: "" });
      setCustomSize(true);
      return;
    }
    const paper = papers?.find((p) => p.name === name);
    if (!paper) return;
    update({
      print_paper_name: paper.name,
      print_short_mm: paper.short_mm,
      print_long_mm: paper.long_mm,
    });
    setCustomSize(false);
  };

  const testPrint = async () => {
    if (!selected) return;
    try {
      await cmd.printPhoto(selected, {
        printer: draft.printer,
        paper: draft.print_paper_name,
        short_mm: draft.print_short_mm,
        long_mm: draft.print_long_mm,
      });
    } catch (error) {
      fail(error);
    }
  };

  const exportConfig = async () => {
    try {
      const path = await cmd.exportCliConfig();
      if (path) setFooterText(`Saved to ${path}`);
    } catch (error) {
      fail(error);
    }
  };

  const extensions = [...new Set([...KNOWN_EXTENSIONS, ...draft.extensions])];
  const knownPrinter = !draft.printer || printers?.some((p) => p.name === draft.printer);
  const knownPaper =
    !draft.print_paper_name || papers?.some((p) => p.name === draft.print_paper_name);
  const remakes = strip > 0 && printsChange(draft, saved);
  const renames = draft.output_suffix !== saved.output_suffix;

  return (
    <Flex direction="column" height="100vh">
      <Tabs.Root
        value={tab}
        onValueChange={(value) => setTab(value as TabName)}
        style={{ display: "flex", flexDirection: "column", flexGrow: 1, minHeight: 0 }}
      >
        <Tabs.List justify="center">
          <Tabs.Trigger value="general">General</Tabs.Trigger>
          <Tabs.Trigger value="print">Print</Tabs.Trigger>
          <Tabs.Trigger value="advanced">Advanced</Tabs.Trigger>
        </Tabs.List>

        <ScrollArea scrollbars="vertical" style={{ flexGrow: 1 }}>
          <Box p="5">
            <Tabs.Content value="general">
              <Flex direction="column" gap="5">
                <Row
                  label="Watch folder"
                  hint="Photos that land here are printed. AirDrop puts them in Downloads."
                  error={errorFor("watch_dir")}
                >
                  <Flex gap="2">
                    <Box flexGrow="1">
                      <TextField.Root value={draft.watch_dir} readOnly />
                    </Box>
                    <Button type="button" variant="soft" onClick={chooseFolder}>
                      Choose…
                    </Button>
                  </Flex>
                </Row>

                <Row
                  label="Kinds of photo to watch for"
                  hint="Anything else that lands in the folder is left alone."
                  error={errorFor("extensions")}
                >
                  <CheckboxCards.Root
                    columns="4"
                    size="1"
                    value={draft.extensions}
                    onValueChange={(value) => update({ extensions: value })}
                  >
                    {extensions.map((ext) => (
                      <CheckboxCards.Item key={ext} value={ext}>
                        {ext.toUpperCase()}
                      </CheckboxCards.Item>
                    ))}
                  </CheckboxCards.Root>
                </Row>
              </Flex>
            </Tabs.Content>

            <Tabs.Content value="print">
              <Flex direction="column" gap="5">
                <Row label="Printer">
                  <Select.Root
                    value={draft.printer || DEFAULT_PRINTER}
                    onValueChange={(value) =>
                      update({ printer: value === DEFAULT_PRINTER ? "" : value })
                    }
                  >
                    <Select.Trigger placeholder="Looking for printers…" />
                    <Select.Content position="popper">
                      <Select.Item value={DEFAULT_PRINTER}>System default</Select.Item>
                      {printers?.map((p) => (
                        <Select.Item key={p.name} value={p.name}>
                          {p.is_default ? `${p.name} (default)` : p.name}
                        </Select.Item>
                      ))}
                      {!knownPrinter && printers && (
                        <Select.Item value={draft.printer} disabled>
                          {`${draft.printer} (not connected)`}
                        </Select.Item>
                      )}
                    </Select.Content>
                  </Select.Root>
                </Row>

                <Row
                  label="Paper"
                  hint="Choosing a paper sets the print size, so the photo is cropped to the paper's shape."
                  error={errorFor("paper")}
                >
                  <Select.Root
                    value={draft.print_paper_name || DRIVER_PAPER}
                    onValueChange={choosePaper}
                    disabled={!papers}
                  >
                    <Select.Trigger placeholder="Reading the printer's papers…" />
                    <Select.Content position="popper">
                      <Select.Item value={DRIVER_PAPER}>Let the printer choose</Select.Item>
                      <Select.Separator />
                      {papers?.map((p) => (
                        <Select.Item key={p.name} value={p.name}>
                          {p.label} - {mm(p.short_mm)} × {mm(p.long_mm)} mm
                          {p.borderless ? " · borderless" : ""}
                        </Select.Item>
                      ))}
                      {!knownPaper && papers && (
                        <Select.Item value={draft.print_paper_name} disabled>
                          {`${draft.print_paper_name} (not on this printer)`}
                        </Select.Item>
                      )}
                    </Select.Content>
                  </Select.Root>

                  <Flex gap="3" align="center" mt="3">
                    <NumberField
                      label="Short side"
                      unit="mm"
                      value={draft.print_short_mm}
                      disabled={!customSize}
                      onChange={(value) => update({ print_short_mm: value })}
                    />
                    <NumberField
                      label="Long side"
                      unit="mm"
                      value={draft.print_long_mm}
                      disabled={!customSize}
                      onChange={(value) => update({ print_long_mm: value })}
                    />
                    <Text as="label" size="2">
                      <Flex gap="2" align="center">
                        <Switch
                          size="1"
                          checked={customSize}
                          onCheckedChange={setCustomSize}
                        />
                        Custom size
                      </Flex>
                    </Text>
                  </Flex>

                  {shapes.length > 0 && (
                    <Callout.Root color="amber" size="1" mt="3">
                      <Callout.Icon>
                        <ExclamationTriangleIcon />
                      </Callout.Icon>
                      {shapes.map((text) => (
                        <Callout.Text key={text}>{text}</Callout.Text>
                      ))}
                    </Callout.Root>
                  )}
                </Row>

                <Row
                  label="Background"
                  hint="Fills whatever the overlay does not cover and the photo does not reach."
                  error={errorFor("background")}
                >
                  <Flex gap="2" align="center">
                    <input
                      type="color"
                      aria-label="Background colour"
                      style={swatch}
                      value={/^#[0-9a-f]{6}$/i.test(draft.background) ? draft.background : "#ffffff"}
                      onChange={(event) => update({ background: event.target.value })}
                    />
                    <TextField.Root
                      value={draft.background}
                      onChange={(event) => update({ background: event.target.value })}
                      style={{ width: "8em" }}
                    />
                  </Flex>
                </Row>

                <Row
                  label="Test print"
                  hint="Prints the selected photo with the printer and paper above, before you save them."
                >
                  <Box>
                    <Button type="button" variant="soft" disabled={!selected} onClick={testPrint}>
                      Print the selected photo
                    </Button>
                  </Box>
                </Row>
              </Flex>
            </Tabs.Content>

            <Tabs.Content value="advanced">
              <Flex direction="column" gap="5">
                <Callout.Root color="gray" size="1">
                  <Callout.Icon>
                    <InfoCircledIcon />
                  </Callout.Icon>
                  <Callout.Text>
                    These keep the photo untouched and stop Snappr printing its own prints.
                    Change them only if you know why.
                  </Callout.Text>
                </Callout.Root>

                <Row
                  label="Print file name ending"
                  hint="Added to the photo's name for the print, so IMG_1234.heic is printed as IMG_1234.overlay.png. It has to end in .png."
                  error={errorFor("output_suffix")}
                >
                  <TextField.Root
                    value={draft.output_suffix}
                    onChange={(event) => update({ output_suffix: event.target.value })}
                  />
                  {renames && (
                    <Text size="1" color="gray" mt="1">
                      Prints already made keep their old names.
                    </Text>
                  )}
                </Row>

                <Row
                  label="PNG compression"
                  hint="0 is fastest and biggest, 9 smallest and slowest. Never changes the picture."
                  error={errorFor("png_compression")}
                >
                  <NumberField
                    integer
                    min={0}
                    value={draft.png_compression}
                    onChange={(value) => update({ png_compression: value })}
                  />
                </Row>

                <Row
                  label="Previews"
                  hint="What the window draws. The print is never made from these."
                  error={errorFor("preview")}
                >
                  <Grid columns="3" gap="3">
                    <NumberField
                      label="Full size"
                      unit="px"
                      integer
                      min={0}
                      value={draft.preview_max_px}
                      onChange={(value) => update({ preview_max_px: value })}
                    />
                    <NumberField
                      label="Thumbnail"
                      unit="px"
                      integer
                      min={0}
                      value={draft.preview_thumb_px}
                      onChange={(value) => update({ preview_thumb_px: value })}
                    />
                    <NumberField
                      label="Quality"
                      integer
                      min={0}
                      value={draft.preview_quality}
                      onChange={(value) => update({ preview_quality: value })}
                    />
                  </Grid>
                </Row>

                <Row
                  label="Arriving photos"
                  hint="AirDrop writes a photo in pieces. Snappr waits until it stops changing this many times in a row, and gives up after the timeout."
                  error={errorFor("stability")}
                >
                  <Grid columns="3" gap="3">
                    <NumberField
                      label="Checks"
                      integer
                      min={0}
                      value={draft.stability_checks}
                      onChange={(value) => update({ stability_checks: value })}
                    />
                    <NumberField
                      label="Timeout"
                      unit="s"
                      integer
                      min={0}
                      value={draft.stability_timeout_secs}
                      onChange={(value) => update({ stability_timeout_secs: value })}
                    />
                  </Grid>
                </Row>

                <Separator size="4" />

                <Flex gap="3">
                  <Button type="button" variant="soft" color="gray" onClick={exportConfig}>
                    Export for the command line…
                  </Button>
                  <Button
                    type="button"
                    variant="soft"
                    color="gray"
                    onClick={() => void cmd.revealSettingsFile().catch(fail)}
                  >
                    Show settings file
                  </Button>
                </Flex>
              </Flex>
            </Tabs.Content>
          </Box>
        </ScrollArea>
      </Tabs.Root>

      <Separator size="4" />
      <Flex direction="column" gap="2" p="4">
        {footerProblem && (
          <Text size="2" color="red">
            {footerProblem}
          </Text>
        )}
        {!footerProblem && footerText && (
          <Text size="2" color="gray">
            {footerText}
          </Text>
        )}
        {!footerProblem && !footerText && remakes && (
          <Text size="2" color="gray">
            {strip === 1
              ? "Saving makes the print on the strip again."
              : `Saving makes the ${strip} prints on the strip again.`}
            {renames ? " The old files stay where they are." : ""}
          </Text>
        )}
        <Flex gap="3" align="center">
          <Button
            type="button"
            variant="ghost"
            color="gray"
            onClick={() => update(resetTab(tab, draft, defaults))}
          >
            Reset this tab
          </Button>
          <Box flexGrow="1" />
          <Button type="button" variant="soft" color="gray" onClick={close}>
            Cancel
          </Button>
          <Button type="button" disabled={!dirty || saving} loading={saving} onClick={save}>
            Save
          </Button>
        </Flex>
      </Flex>
    </Flex>
  );
}

/** A label, the control, a hint under it, and the refusal when it is this field's. */
function Row({
  label,
  hint,
  error,
  children,
}: {
  label: string;
  hint?: string;
  error?: string | null;
  children: ReactNode;
}) {
  return (
    <Flex direction="column" gap="1">
      <Text size="2" weight="medium">
        {label}
      </Text>
      {children}
      {error ? (
        <Text size="1" color="red">
          {error}
        </Text>
      ) : (
        hint && (
          <Text size="1" color="gray">
            {hint}
          </Text>
        )
      )}
    </Flex>
  );
}

/**
 * A number in a text field.
 *
 * Holds its own text, so a half-typed value ("1", "10", "", "-") is allowed on
 * the way to a real one, and the draft only ever receives a real number. Text
 * that is not a number goes back to the last good value when the field loses
 * focus - which is why the window never has to say "that is not a number".
 */
function NumberField({
  label,
  unit,
  value,
  onChange,
  integer = false,
  min,
  disabled = false,
}: {
  label?: string;
  unit?: string;
  value: number;
  onChange: (value: number) => void;
  integer?: boolean;
  min?: number;
  disabled?: boolean;
}) {
  const [text, setText] = useState(String(value));
  useEffect(() => setText(String(value)), [value]);

  const parse = (raw: string) => {
    const parsed = Number(raw.trim());
    if (raw.trim() === "" || !Number.isFinite(parsed)) return null;
    if (integer && !Number.isInteger(parsed)) return null;
    if (min !== undefined && parsed < min) return null;
    return parsed;
  };

  return (
    <Flex direction="column" gap="1">
      {label && (
        <Text size="1" color="gray">
          {label}
        </Text>
      )}
      <TextField.Root
        inputMode={integer ? "numeric" : "decimal"}
        value={text}
        disabled={disabled}
        onChange={(event) => {
          setText(event.target.value);
          const parsed = parse(event.target.value);
          if (parsed !== null) onChange(parsed);
        }}
        onBlur={() => setText(String(value))}
        style={{ width: "7em" }}
      >
        {unit && <TextField.Slot side="right">{unit}</TextField.Slot>}
      </TextField.Root>
    </Flex>
  );
}
