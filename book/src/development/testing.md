# Semantic tests

Tests should verify semantic process boundaries, not only final values. Important
cases include failure deltas, bounce selection, rollback, compact trajectories,
multi-tool executor frame counts, internal/external tool-result equivalence, and
equivalence between `yolo` and repeated `step` evaluation. Assert that runtime
failure adds exactly one frame and that rollback restores the unchanged parent.
