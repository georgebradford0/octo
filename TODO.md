# TODO

## Known Limitations

- [ ] **Oversized individual turns**: A single turn containing an enormous tool result (e.g. a large file read) can itself exceed the context limit. Rotation won't help since the turn can't be split. Needs chunking or truncation of tool result content before it enters history.
- [ ] Setup push notifications on mobile to let user know when something is finished.
- [ ] Yes — Unicode braille patterns (⠿) encode a 2×4 dot grid per character, so you get 8 QR modules per
  character instead of 2 with half-blocks. That's 4× shorter height and 2× narrower width vs ANSIUTF8.
