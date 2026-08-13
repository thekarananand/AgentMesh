# Clipboard Mojibake — Investigation Findings

## Verdict: not reproducible, no actionable bug. Closed.

**Confirmed end-to-end in the packaged app, not just dev mode or isolated test scripts:** a
real Cmd+C out of a terminal tab in the installed `AgentMesh.app`, Cmd+V'd back into this same
conversation, round-tripped a stress block covering accented Latin, three CJK scripts, ZWJ and
skin-tone emoji, combining diacritics, and mixed punctuation (em/en dash, smart quotes,
ellipsis) — byte-for-byte clean. No corruption anywhere in the chain a real user actually uses.

**The reported corruption does not exist on the pasteboard.** Copying non-ASCII text out of an
AgentMesh terminal puts byte-perfect UTF-8 on the macOS pasteboard, and the API every modern
Cocoa app uses to read a plain-text paste returns it intact — accented Latin, CJK and emoji alike.

The "corrupt legacy flavor" that an earlier pass of this investigation blamed on Electron is a
**reader-side artifact of `osascript -e 'clipboard info'`**, a Carbon-era tool that asks the OS to
*synthesize* pasteboard representations that were never written. No code change was made, and none
is warranted. The three "recommended next steps" this document used to carry — file upstream, pin
a different Electron, layer flavors with `writeBuffer` — are all dead ends and are documented as
such below so nobody spends another cycle on them.

## The original symptom claim

Text copied out of an AgentMesh terminal tab and pasted elsewhere (TextEdit, Notes, back into a
Claude Code prompt) was reported to come out corrupted for any non-ASCII character, with a
recognizable mojibake signature — UTF-8 bytes decoded as **MacRoman**: `café — naïve` →
`caf√© ‚Äî na√Øve`.

That claim was never confirmed against a real paste. All of the evidence behind it came from two
Carbon-era CLI tools, `pbpaste` and `osascript -e 'clipboard info'`, which have their own
flavor-preference rules and are not what a GUI app's Cmd+V goes through.

## The decisive test

The question that decided the whole investigation: **does the API real GUI apps use to read a
plain-text paste return corrupt text?** That API is

```swift
NSPasteboard.general.string(forType: NSPasteboard.PasteboardType.string)
```

Checking it directly sidesteps GUI automation and Accessibility permissions entirely. Swift ships
with the Xcode command line tools, so `swift <file>.swift` is enough; there is no PyObjC on this
machine (`python3 -c "import AppKit"` fails).

Two test strings, chosen to catch a fix that trades one bug for another:

- `café — naïve` — accented Latin plus an em-dash; the original claim's victim.
- `日本語 🎉` — CJK and emoji, which MacRoman **cannot represent at all**.

And two writers, because the app's real copy path is a renderer DOM `copy` event (xterm.js does
`clipboardData.setData('text/plain', …)`), which goes through a different Chromium code path from
the main-process API:

- main process: `clipboard.writeText(text)`
- renderer: a real `copy` event with `setData('text/plain', …)`, in a focused `BrowserWindow`

The pasteboard was poisoned with a `SENTINEL` string before every write, so a write that silently
failed could not be mistaken for a pass. (This matters: the first renderer attempt used
`show: false`, and Chromium refused `execCommand('copy')` without an active document — the
SENTINEL survived and caught it.)

### Result — every case correct

| Writer | Text | `string(forType: .string)` | Real `'TEXT'` flavor on pasteboard | `clipboard info` says `string,` |
|---|---|---|---|---|
| main `writeText` | `café — naïve` | `café — naïve` ✅ 16 B | **absent** | 3 |
| main `writeText` | `日本語 🎉` | `日本語 🎉` ✅ 14 B | **absent** | 0 |
| renderer `copy` event | `café — naïve` | `café — naïve` ✅ 16 B | **absent** | 3 |
| renderer `copy` event | `日本語 🎉` | `日本語 🎉` ✅ 14 B | **absent** | 0 |

Byte-exact in all four cases: `63 61 66 C3 A9 20 E2 80 94 20 6E 61 C3 AF 76 65` and
`E6 97 A5 E6 9C AC E8 AA 9E 20 F0 9F 8E 89`. `readObjects(forClasses: [NSString])` — the other
common read path — agrees. `pbpaste | xxd` is also correct, and always was.

## What Electron actually puts on the pasteboard

This is the finding that closes the case. Enumerating `NSPasteboard.general.types` raw and
unfiltered after an Electron write returns **exactly one text flavor**:

```
public.utf8-plain-text                  -> 16 bytes   café — naïve
NSStringPboardType                      -> 16 bytes   (same UTI — an alias, not a second flavor)
org.chromium.internal.source-rfh-token  -> 24 bytes   (provenance, renderer path only)
org.chromium.source-url                 -> …          (provenance, renderer path only)
```

Two things to note:

- **`NSStringPboardType` is not the legacy flavor.** In modern AppKit its raw value *is*
  `public.utf8-plain-text`, and so is `NSPasteboard.PasteboardType.string`'s. Probing it proves
  nothing about legacy behavior. The genuine legacy Carbon flavor is `'TEXT'`, which must be
  probed explicitly as `CorePasteboardFlavorType 0x54455854`.
- **There is no `'TEXT'` flavor, no `'utxt'`, and no UTF-16 flavor of any kind.** Electron writes
  one representation and nothing else.

So the `«class ut16»`, `string` and `Unicode text` entries that `clipboard info` reports after an
Electron write describe **representations that are not on the pasteboard**. They are manufactured
on demand by the Carbon compatibility coercion layer when `clipboard info` asks for them. Two of
the three are synthesized correctly; `string` is not.

There is nothing sitting on the pasteboard for any app to read wrong.

## Where `string, 3` came from — and it truncates, it does not mojibake

The earlier pass read `string, 3` for a 4-character `café` and concluded the flavor was "short by
exactly the number of multi-byte characters." That generalization was an artifact of a single
4-character test case. The real rule, measured:

| Text | Leading ASCII-only prefix | `clipboard info` `string,` |
|---|---|---|
| `café` | 3 (`caf`) | 3 |
| `café — naïve` | 3 (`caf`) | 3 |
| `ABCDEFGHIJé` | 10 | 10 |
| `ABéCD` | 2 | 2 |
| `éABCDEFGH` | 0 | 0 |
| `日本語 🎉` | 0 | 0 |

The byte count is **exactly the length of the leading ASCII prefix**, every time. The coercion
stops dead at the first non-ASCII byte. Note `café` and `café — naïve` both give 3 — a constant,
not something scaling with character count.

**This is truncation, not mojibake, and that distinction falsifies the original diagnosis.** The
reported symptom was `caf√© ‚Äî na√Øve` — every character present, wrongly decoded. The flavor
that was blamed produces `caf` — characters *missing*. The two signatures are mutually exclusive.
The flavor cannot have caused the symptom attributed to it.

Where the reported signature *does* come from: rendering the **correct** UTF-8 bytes as MacRoman.
The Swift probe prints both decodings of every flavor, and the byte-perfect
`public.utf8-plain-text` flavor decodes under MacRoman as exactly `caf√© ‚Äî na√Øve` (and
`日本語 🎉` as `Êó•Êú¨Ë™û üéâ`). That is a **misinterpreting reader or display**, not corrupt
clipboard data — the bytes are right; something downstream decoded them with the wrong encoding.

## Dead ends — confirmed closed, do not retry

1. **File upstream against Electron / Chromium.** Nothing to file. Electron writes one correct
   flavor; the bad byte count is produced inside Apple's coercion layer at read time, by a tool
   asking for a representation nobody wrote.
2. **Pin a different Electron version.** Tested 43.3.0 (this repo's pin), 43.4.0 (latest patch)
   and 28.3.3 (a ~2-year-old major) via `npx electron@<version>`, each through the full `types`
   probe: **byte-identical in all three** — same correct 16 bytes, same single declared flavor,
   `'TEXT'` absent everywhere. Not a recent regression; version pinning changes nothing.
3. **Layer a corrected legacy flavor in with `clipboard.writeBuffer()`.** Actively harmful, not
   merely ineffective. `writeBuffer(format, buffer)` is **destructive**: each call replaces the
   *entire* pasteboard with just that one format. Sequential calls do not combine — only the last
   survives. Measured, running exactly the sequence this step proposed (`writeText`, then
   `writeBuffer` for the good flavor, then `writeBuffer` for a hand-built correct MacRoman
   `'TEXT'`):

   ```
   clipboard info : string, 12
   declared types : com.apple.traditional-mac-plain-text -> 12 B  63 61 66 8E 20 D1 20 6E 61 95 76 65
   string(forType: .string) : nil        ← plain-text paste now returns NOTHING
   ```

   The legacy flavor is the *only* thing left on the pasteboard and `public.utf8-plain-text` is
   gone, so the API every modern app pastes through returns `nil`. This "fix" would have broken
   copy/paste outright for every app, in order to satisfy `clipboard info`.

## Why routing the copy through AppleScript would be a net regression

`osascript -e 'set the clipboard to …'` does write a real legacy `'TEXT'` flavor, and for
accented Latin it is correctly MacRoman-encoded (`café — naïve` → `63 61 66 8E 20 D1 20 6E 61 95
76 65`, verified byte-for-byte). That was the leading candidate fix. It was **not implemented**,
and the same probe run against the CJK string shows why:

```
AppleScript, 日本語 🎉
  public.utf8-plain-text                 -> 14 B  E6 97 A5 E6 9C AC E8 AA 9E 20 F0 9F 8E 89   ✅
  CorePasteboardFlavorType 0x54455854    ->  8 B  93 FA 96 7B 8C EA 20 3F                     ❌
```

That legacy flavor is **Shift-JIS** (`93FA` 日, `967B` 本, `8CEA` 語) — the system's legacy script
encoding, not MacRoman — and the emoji has become `3F`, a literal **`?`**. Irreversible data loss,
newly introduced, on a path that is currently correct.

So the "fix" would add a lossy representation to the pasteboard in order to satisfy a reader that
nothing in the reported symptom actually implicates, and would trade a mojibake bug that does not
reproduce for a data-loss bug that does. It also drags along a `styled Clipboard text` rich-text
side-flavor (`CorePasteboardFlavorType 0x7374796C`) that Electron never writes and that a
plain-text terminal copy has no business declaring.

## What was ruled out along the way

- **OS-level clipboard round trip** (`pbcopy` / `pbpaste`) — clean, confirmed again here.
- **node-pty's read/write encoding** — `utf8` in both directions (`Buffer.from(data, 'utf8')`).
- **xterm.js's paste handler** — vanilla `clipboardData.getData('text/plain')`.
- **xterm.js's copy handler** — vanilla `clipboardData.setData('text/plain', …)`.
- **OSC 52** — not implemented at all in the bundled xterm.js 5.3.0; no handler exists.
- **This repo's own code** — `main.js`, `preload.js` and `renderer.js` contain **zero**
  clipboard-related code (`grep -rn -e clipboard -e writeText -e execCommand` over the tracked
  `.js`/`.html` returns nothing). There is no interception point to fix even if there were a bug.
- **Shell locale (`LANG`/`LC_ALL`/`LC_CTYPE`) inside the pty** — was empty/`C`, which looked
  suspicious, but typing an accented character directly (no paste involved) came through clean
  under the same locale. See the separate section below.

## Reproducing this check

Both harnesses live outside the repo (scratchpad, not committed); they are ~20 lines each and
trivial to recreate:

1. A Swift file that dumps `NSPasteboard.general.string(forType: .string)`, then enumerates
   `pb.types` **raw and unfiltered** with `pb.data(forType:)` hexdumped for each, then probes the
   legacy flavors explicitly by name — `CorePasteboardFlavorType 0x54455854` (`'TEXT'`),
   `0x75747874` (`'utxt'`), `com.apple.traditional-mac-plain-text`. Decode every flavor as both
   UTF-8 and `.macOSRoman` so a wrongly-encoded one is readable on sight.
2. An Electron script that writes the clipboard, with `setTimeout(…, 800)` before `app.quit()` so
   the pasteboard write can't race process teardown.

Non-obvious mechanics, each of which cost a cycle:

- **Poison the pasteboard with a sentinel before every write.** A write that silently no-ops
  otherwise reads as a pass.
- **A renderer copy needs a shown, focused window.** `show: false` makes Chromium refuse
  `execCommand('copy')`.
- **Dump `types` unfiltered.** The whole conclusion rests on which flavors are *absent*; a filter
  that hides unrecognized entries hides the answer.
- **Never trust `clipboard info` as ground truth.** It is the tool that manufactured this bug
  report.

## If the symptom is ever reported again

Do not re-run `clipboard info`. Check, in this order:

1. `NSPasteboard.general.string(forType: .string)` via the Swift probe, on the real copy. If that
   is correct, the clipboard is correct and the problem is downstream of it.
2. The **paste target's** own decoding. The reported signature is what a reader does to correct
   UTF-8 when it assumes MacRoman — so suspect whatever consumed the text, including a `claude`
   CLI running under a `C` locale (see below).
3. Whether the text was ever actually pasted, versus eyeballed through `pbpaste` /
   `clipboard info` in a terminal whose own encoding settings are in play.

## A separate, unrelated finding: empty locale in the spawned environment

While investigating, `LANG`/`LC_ALL`/`LC_CTYPE` were found completely unset (`C` locale) in the
live AgentMesh-hosted session, versus a real terminal on the same machine which had `en_IN.UTF-8`.
This is a real, known class of bug: GUI apps launched via Finder/Dock (as opposed to a shell)
don't source the login shell's rc files, so `LANG` often arrives unset in their process tree.

**This is not the fix for the mojibake above** — it was tested and falsified as the paste-input
mechanism (typed accented input was clean under the same empty locale), and now that the clipboard
itself is confirmed correct there is no mojibake left for it to explain. It is worth having fixed
on portability grounds alone (this project's own standing rule: "Portability is a standing
requirement"). `main.js` now defaults `LANG`/`LC_ALL` to `en_US.UTF-8` if unset, before `ptyEnv`
is captured, so both Electron's own process tree and every spawned pty inherit a sane locale
instead of silently falling back to ASCII-only `C`.

This change has **not been verified against a running build** — testing it required either
rebuilding the packaged app or restarting the live window (which hosts the conversation that made
the change), and neither was done. Treat it as applied-but-unverified hygiene, tracked separately
from the clipboard question.

To be clear about its status: this is unverified hygiene, **not** a candidate explanation for the
mojibake claim. It was falsified as one above — typed accented input was clean under the same empty
locale — and nothing here reinstates it. Step 2 of the checklist above covers the
downstream-decoder angle without needing to nominate a suspect.
