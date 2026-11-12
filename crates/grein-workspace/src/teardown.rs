#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeardownAction {
    AutoPruneClean,
    Keep,
    Discard,
    Merge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeardownOutcome {
    Pruned,
    Kept,
    Discarded,
    Merged,
}
