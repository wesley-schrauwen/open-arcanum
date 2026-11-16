# TODO

Things to pick up later. Larger plans live in `plans/`; project state and findings in `NOTES.md`.

## Art

- **Loading screen pocket watch:** the hand steps visibly because `LoadIndicator.ART` is 30
  pre-drawn frames (12° apart), and at one turn per 8 s each frame shows for ~0.27 s. The owner
  will make a replacement (GIF or animated SVG) with a smoothly rotating hand; the loading screen
  (`loading.rs`, `LoadingIndicator`) then needs to play it instead of the `.art` frames.

## Rules (after the faithful copy)

- **Training rank survives losing the skill:** the original never lowers a skill's training rank
  when its points drop (`NOTES.md` → Character rules → Copied oddities), so a Master can sit on a
  skill level below Master's 18. Likely left in because lowering a rank has wider effects: the
  rank's abilities, trainer and quest state, and refunds. Fix by dropping the rank to the highest
  one the new level still meets ([0,1,9,18]), and decide what happens to anything the rank paid
  for or unlocked.
