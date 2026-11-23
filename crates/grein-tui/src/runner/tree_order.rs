use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::app::{App, ThreadItem};
use crate::engine_port::ThreadMeta;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderedThreadEntry {
    pub index: usize,
    pub depth: usize,
    pub prefix: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldedThreadEntry {
    pub index: usize,
    pub depth: usize,
    pub prefix: String,
    pub has_children: bool,
    pub hidden_descendants: usize,
}

struct FoldedTreeOrderCtx<'a> {
    threads: &'a [ThreadItem],
    children: HashMap<&'a str, Vec<usize>>,
    collapsed: &'a BTreeSet<String>,
    visited: Vec<bool>,
    ordered: Vec<FoldedThreadEntry>,
}

impl<'a> FoldedTreeOrderCtx<'a> {
    fn count_hidden_descendants(&mut self, idx: usize) -> usize {
        let mut count = 0usize;
        let Some(child_indices) = self.children.get(self.threads[idx].id.as_str()).cloned() else {
            return 0;
        };
        for child_idx in child_indices {
            if !self.visited[child_idx] {
                self.visited[child_idx] = true;
                count += 1;
                count += self.count_hidden_descendants(child_idx);
            }
        }
        count
    }

    fn visit(
        &mut self,
        idx: usize,
        depth: usize,
        prefix: String,
        ancestor_continuations: &[bool],
    ) {
        if self.visited[idx] {
            return;
        }
        self.visited[idx] = true;
        let pending_children: Vec<usize> = self
            .children
            .get(self.threads[idx].id.as_str())
            .map(|kids| kids.iter().copied().filter(|&c| !self.visited[c]).collect())
            .unwrap_or_default();
        let has_children = !pending_children.is_empty();
        let is_collapsed = has_children && self.collapsed.contains(&self.threads[idx].id);
        let entry_pos = self.ordered.len();
        self.ordered.push(FoldedThreadEntry {
            index: idx,
            depth,
            prefix,
            has_children,
            hidden_descendants: 0,
        });
        if is_collapsed {
            let hidden = self.count_hidden_descendants(idx);
            self.ordered[entry_pos].hidden_descendants = hidden;
            return;
        }
        let total = pending_children.len();
        for (pos, child_idx) in pending_children.into_iter().enumerate() {
            let is_last = pos + 1 == total;
            let mut child_prefix = String::new();
            for &has_more in ancestor_continuations {
                child_prefix.push_str(if has_more { "│  " } else { "   " });
            }
            child_prefix.push_str(if is_last { "└─ " } else { "├─ " });

            let mut next_continuations = ancestor_continuations.to_vec();
            next_continuations.push(!is_last);
            self.visit(child_idx, depth + 1, child_prefix, &next_continuations);
        }
    }
}

/// Orders threads in parent-child depth-first order while hiding descendants of folded threads.
pub fn compute_ordered_thread_entries_with_folds(
    threads: &[ThreadItem],
    thread_meta: &HashMap<String, ThreadMeta>,
    collapsed: &BTreeSet<String>,
) -> Vec<FoldedThreadEntry> {
    let known_ids: HashSet<&str> = threads.iter().map(|t| t.id.as_str()).collect();
    let mut children: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut roots: Vec<usize> = Vec::new();

    for (idx, thread) in threads.iter().enumerate() {
        let parent = thread_meta
            .get(&thread.id)
            .and_then(|m| m.parent_thread_id.as_deref());
        if let Some(pid) = parent {
            if pid != thread.id && known_ids.contains(pid) {
                children.entry(pid).or_default().push(idx);
                continue;
            }
        }
        roots.push(idx);
    }

    let mut ctx = FoldedTreeOrderCtx {
        threads,
        children,
        collapsed,
        visited: vec![false; threads.len()],
        ordered: Vec::with_capacity(threads.len()),
    };
    for root_idx in roots {
        ctx.visit(root_idx, 0, String::new(), &[]);
    }
    for idx in 0..threads.len() {
        if !ctx.visited[idx] {
            ctx.visit(idx, 0, String::new(), &[]);
        }
    }
    ctx.ordered
}

struct TreeOrderCtx<'a> {
    threads: &'a [ThreadItem],
    children: HashMap<&'a str, Vec<usize>>,
    visited: Vec<bool>,
    ordered: Vec<OrderedThreadEntry>,
}

impl<'a> TreeOrderCtx<'a> {
    fn visit(
        &mut self,
        idx: usize,
        depth: usize,
        prefix: String,
        ancestor_continuations: &[bool],
    ) {
        if self.visited[idx] {
            return;
        }
        self.visited[idx] = true;
        self.ordered.push(OrderedThreadEntry {
            index: idx,
            depth,
            prefix,
        });
        let Some(child_indices) = self.children.get(self.threads[idx].id.as_str()) else {
            return;
        };
        let pending_children: Vec<usize> = child_indices
            .iter()
            .copied()
            .filter(|&c| !self.visited[c])
            .collect();
        let total = pending_children.len();
        for (pos, child_idx) in pending_children.into_iter().enumerate() {
            let is_last = pos + 1 == total;
            let mut child_prefix = String::new();
            for &has_more in ancestor_continuations {
                child_prefix.push_str(if has_more { "│  " } else { "   " });
            }
            child_prefix.push_str(if is_last { "└─ " } else { "├─ " });

            let mut next_continuations = ancestor_continuations.to_vec();
            next_continuations.push(!is_last);
            self.visit(child_idx, depth + 1, child_prefix, &next_continuations);
        }
    }
}

/// Orders threads in parent-child depth-first order along with nesting depth and box-drawing prefix.
pub fn compute_ordered_thread_entries(
    threads: &[ThreadItem],
    thread_meta: &HashMap<String, ThreadMeta>,
) -> Vec<OrderedThreadEntry> {
    let known_ids: HashSet<&str> = threads.iter().map(|t| t.id.as_str()).collect();
    let mut children: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut roots: Vec<usize> = Vec::new();

    for (idx, thread) in threads.iter().enumerate() {
        let parent = thread_meta
            .get(&thread.id)
            .and_then(|m| m.parent_thread_id.as_deref());
        if let Some(pid) = parent {
            if pid != thread.id && known_ids.contains(pid) {
                children.entry(pid).or_default().push(idx);
                continue;
            }
        }
        roots.push(idx);
    }

    let mut ctx = TreeOrderCtx {
        threads,
        children,
        visited: vec![false; threads.len()],
        ordered: Vec::with_capacity(threads.len()),
    };
    for root_idx in roots {
        ctx.visit(root_idx, 0, String::new(), &[]);
    }
    for idx in 0..threads.len() {
        if !ctx.visited[idx] {
            ctx.visit(idx, 0, String::new(), &[]);
        }
    }
    ctx.ordered
}

/// Orders threads in parent-child depth-first order along with their nesting depth.
pub fn compute_ordered_thread_indices(
    threads: &[ThreadItem],
    thread_meta: &HashMap<String, ThreadMeta>,
) -> Vec<(usize, usize)> {
    compute_ordered_thread_entries(threads, thread_meta)
        .into_iter()
        .map(|entry| (entry.index, entry.depth))
        .collect()
}

pub fn rebuild_tree_nodes(app: &mut App) {
    let ordered = compute_ordered_thread_entries_with_folds(
        &app.threads,
        &app.thread_meta,
        &app.collapsed_threads,
    );
    let mut nodes = Vec::with_capacity(ordered.len());
    let mut depths = BTreeMap::new();
    let mut prefixes = BTreeMap::new();
    let mut has_children_set = BTreeSet::new();
    let mut hidden_counts = BTreeMap::new();

    for entry in ordered {
        let thread = &app.threads[entry.index];
        depths.insert(thread.id.clone(), entry.depth);
        prefixes.insert(thread.id.clone(), entry.prefix);
        if entry.has_children {
            has_children_set.insert(thread.id.clone());
        }
        if entry.hidden_descendants > 0 {
            hidden_counts.insert(thread.id.clone(), entry.hidden_descendants);
        }
        nodes.push(crate::ui::tree::TreeNode::Thread(
            crate::ui::tree::ThreadNode {
                id: thread.id.clone(),
                title: thread.title.clone(),
            },
        ));
    }
    app.tree_has_children = has_children_set;
    app.tree_hidden_counts = hidden_counts;

    let prev_selected_id = app
        .tree
        .selected_index()
        .and_then(|idx| app.tree.nodes.get(idx))
        .map(|node| match node {
            crate::ui::tree::TreeNode::Thread(t) => t.id.clone(),
            crate::ui::tree::TreeNode::Anchor(a) => a.thread_id.clone(),
        });
    let prev_selected = app.tree.selected_index();
    let prev_scroll = app.tree.scroll_offset;
    let prev_active = app.tree.active_anchor_id().map(str::to_string);
    let node_count = nodes.len();

    app.tree = crate::ui::tree::TreeState::new(nodes);
    app.tree.set_depths(depths);
    app.tree.set_prefixes(prefixes);
    app.tree.set_scroll_offset(prev_scroll);

    if node_count > 0 {
        let restored_idx = prev_selected_id
            .and_then(|id| {
                app.tree.nodes.iter().position(|n| {
                    matches!(n, crate::ui::tree::TreeNode::Thread(t) if t.id == id)
                })
            })
            .or_else(|| prev_selected.map(|i| i.min(node_count - 1)));
        if let Some(idx) = restored_idx {
            app.tree.select_index(idx);
        }
    }
    if let Some(active_id) = prev_active {
        app.tree.set_active_anchor_id(Some(active_id));
    }
}
