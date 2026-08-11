# Reading the CLI's Internals

Everything undocumented in AgentMesh documentation was read out of the shipped binary, not 
guessed. The method, because it will be needed again on the next version bump:

## Extraction process

```bash
readlink -f "$(which claude)"        # e.g. $(brew --prefix)/Caskroom/claude-code@latest/<ver>/claude
strings -n 6 <that path> > cc-strings.txt   # ~280MB Mach-O, bun-compiled; ~2 min, 420k lines
```

The bundled JS survives as a handful of enormous single lines, so `grep` is useless for reading
it — match a needle and print a window of surrounding characters instead (a ~15-line Node 
script doing `indexOf` + `slice`).

## Finding entry points

Log strings are the best entry points: they're distinctive, they sit inside the function 
that implements the thing, and they survive minification when identifiers don't.

Example: `[uds-messaging] Unhandled control action:` led to the whole rename protocol 
documented in [04-session-rename.md](04-session-rename.md).

## Verification

**Anything found this way gets verified against a live session** before it's built on — 
spawn a throwaway `claude` in a pty, drive it, and read the files back. 

See [09-local-dev-testing.md](09-local-dev-testing.md) for testing methodology.
