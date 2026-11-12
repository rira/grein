use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TaskPriority {
    Background = 0,
    Focused = 1,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledTask {
    pub id: u64,
    pub thread_id: String,
    pub priority: TaskPriority,
}

#[derive(Default)]
pub struct PriorityScheduler {
    focused_thread_id: Option<String>,
    tasks: Vec<ScheduledTask>,
}

impl PriorityScheduler {
    pub fn new() -> Self {
        Self {
            focused_thread_id: None,
            tasks: Vec::new(),
        }
    }

    pub fn set_focused_thread(&mut self, thread_id: impl Into<String>) {
        self.focused_thread_id = Some(thread_id.into());
    }

    pub fn enqueue(&mut self, id: u64, thread_id: impl Into<String>) {
        let thread_id = thread_id.into();
        let priority = match &self.focused_thread_id {
            Some(focused) if focused == &thread_id => TaskPriority::Focused,
            _ => TaskPriority::Background,
        };
        self.tasks.push(ScheduledTask {
            id,
            thread_id,
            priority,
        });
    }

    pub fn pop_next(&mut self) -> Option<ScheduledTask> {
        if self.tasks.is_empty() {
            return None;
        }

        // Find highest priority task index (FIFO within same priority)
        let max_priority = self.tasks.iter().map(|t| t.priority).max()?;
        let index = self.tasks.iter().position(|t| t.priority == max_priority)?;
        Some(self.tasks.remove(index))
    }
}

#[test]
fn test_focused_thread_receives_scheduling_priority_over_background_tasks() {
    let mut scheduler = PriorityScheduler::new();
    scheduler.set_focused_thread("thread-focused");

    // Enqueue background tasks first
    scheduler.enqueue(1, "thread-bg-1");
    scheduler.enqueue(2, "thread-bg-2");

    // Enqueue focused task later
    scheduler.enqueue(3, "thread-focused");
    scheduler.enqueue(4, "thread-bg-3");
    scheduler.enqueue(5, "thread-focused");

    // Verify focused thread tasks are scheduled ahead of background tasks
    let first = scheduler.pop_next().expect("Task expected");
    assert_eq!(first.id, 3);
    assert_eq!(first.priority, TaskPriority::Focused);
    assert_eq!(first.thread_id, "thread-focused");

    let second = scheduler.pop_next().expect("Task expected");
    assert_eq!(second.id, 5);
    assert_eq!(second.priority, TaskPriority::Focused);
    assert_eq!(second.thread_id, "thread-focused");

    // Remaining background tasks are popped in FIFO order
    let third = scheduler.pop_next().expect("Task expected");
    assert_eq!(third.id, 1);
    assert_eq!(third.priority, TaskPriority::Background);

    let fourth = scheduler.pop_next().expect("Task expected");
    assert_eq!(fourth.id, 2);
    assert_eq!(fourth.priority, TaskPriority::Background);

    let fifth = scheduler.pop_next().expect("Task expected");
    assert_eq!(fifth.id, 4);
    assert_eq!(fifth.priority, TaskPriority::Background);

    assert!(scheduler.pop_next().is_none());
}

#[test]
fn test_switching_focus_dynamically_prioritizes_new_focused_tasks() {
    let mut scheduler = PriorityScheduler::new();
    scheduler.set_focused_thread("thread-A");

    scheduler.enqueue(10, "thread-B");
    scheduler.enqueue(11, "thread-A");

    // Switch focus to thread B
    scheduler.set_focused_thread("thread-B");
    scheduler.enqueue(12, "thread-B");

    // Thread A task had already been enqueued with Focused priority
    // Thread B task 12 is enqueued with Focused priority
    // Thread B task 10 remains Background
    let first = scheduler.pop_next().expect("First task");
    assert_eq!(first.id, 11);
    assert_eq!(first.priority, TaskPriority::Focused);

    let second = scheduler.pop_next().expect("Second task");
    assert_eq!(second.id, 12);
    assert_eq!(second.priority, TaskPriority::Focused);

    let third = scheduler.pop_next().expect("Third task");
    assert_eq!(third.id, 10);
    assert_eq!(third.priority, TaskPriority::Background);
}
