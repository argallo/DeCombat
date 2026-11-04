# UDP_SESSION_2009_B disasm addendum: APPLIED (stub)

Applied on 2026-10-24 into `UDP_SESSION_2009_B.md` (branch spec-fold), items B1-B25. The full patch text stays in git history
(master 9a73aa3, `specs/UDP_SESSION_2009_B_addendum_disasm.md`). Each item's replacement text is now part of the base spec; each item's grade / evidence
paragraph is kept in that spec's "Appendix Z". Grades were not changed. Master text quoted by an item was matched
verbatim (one match per item) before it was replaced.

Applied: B1-B24 and G1 (G1 went into `GLOSSARY_UDP.md`, the `C2S_SOUND_ACK` row, not into part B)

Not applied, and why:
- B25 (`decomp_sig` identifiers): informational, it quotes no master text. Its result is kept here: policy is numbers only
  from `decomp_sig` (CLEANROOM section 2); the three functions that had existed only there were re-read in the disassembly
  (B12, B15-B17); an identifier check found 0 hits of signature-derived identifiers in the four UDP specs. Part B's
  header lines that still name `decomp_sig` as a numbers-only source are correct and were left as they are.
