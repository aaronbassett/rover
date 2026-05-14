//! Scheduler stub. Real body lands in Task 5.

use tokio::sync::mpsc;

use crate::tasks::types::TaskId;

pub type NewTaskSender = mpsc::UnboundedSender<TaskId>;
#[allow(dead_code)] // constructed in Task 5
pub type NewTaskReceiver = mpsc::UnboundedReceiver<TaskId>;

#[allow(dead_code)] // constructed in Task 5
pub struct Scheduler;
