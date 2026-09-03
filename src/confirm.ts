/**
 * The only file that spells a question put to the operator.
 *
 * The same discipline as `ipc.ts`, for the same reason: these are strings on the
 * wire, so they all live in one place a reader is looking. What it replaces is
 * one inline `confirm(...)` in `App.tsx` that carried its own labels and its own
 * severity, beside two destructive gestures that asked nothing at all - so
 * "does snappr confirm this?" had three different answers and no single place to
 * read them off.
 *
 * **Only the three strings live here.** The severity, the Cancel label and the
 * sheet itself are fixed in Rust's `ask`, which says why - a call site free to
 * pick those is a call site that eventually picks them differently.
 *
 * Three rules the wording follows, all from "What the app is allowed to say" in
 * `docs/desktop-app.md`:
 *
 * 1. Plain sentences. No type name, no path, and none of this repo's own
 *    vocabulary.
 * 2. **Each one says what is *not* lost.** None of these three touch a file the
 *    operator can see, and there is no way for them to know that from the verb
 *    alone - "Clear" and "Remove" both read like deletion.
 * 3. The confirm button repeats the verb from the menu item it was reached by,
 *    never a bare "OK". The operator should be able to answer without re-reading
 *    the question.
 */

import { cmd } from "./ipc";

type Prompt = { title: string; question: string; confirm: string };

const PROMPTS = {
  clearAll: (count: number): Prompt => ({
    title: "Clear the filmstrip",
    question: `Clear ${count} photo${count === 1 ? "" : "s"} from the filmstrip? The photos and any prints already made stay on disk.`,
    confirm: "Clear All",
  }),

  clearPhoto: (file: string): Prompt => ({
    title: "Clear this photo",
    question: `Clear ${file} from the filmstrip? The photo and its print stay on disk.`,
    confirm: "Clear",
  }),

  removeOverlay: (name: string): Prompt => ({
    title: "Remove this overlay",
    question: `Remove ${name} from the overlays? The design file stays where it is, and any orientation using it will print with no overlay.`,
    confirm: "Remove",
  }),
};

type Prompts = typeof PROMPTS;

/**
 * Puts one of the questions above to the operator. True if they agreed.
 *
 * The arguments are typed against the prompt named, so `ask("clearAll", "x")`
 * is a compile error rather than a sheet reading "Clear x photos".
 */
export function ask<K extends keyof Prompts>(
  kind: K,
  ...args: Parameters<Prompts[K]>
): Promise<boolean> {
  // The cast is the price of the lookup: TypeScript will not narrow a union of
  // function types against the key that selected it, even though the signature
  // above has already checked the pair.
  const build = PROMPTS[kind] as (...a: Parameters<Prompts[K]>) => Prompt;
  const { title, question, confirm } = build(...args);
  // Never `invoke` here: `ipc.ts` is the only file that names a command.
  return cmd.ask(title, question, confirm);
}
