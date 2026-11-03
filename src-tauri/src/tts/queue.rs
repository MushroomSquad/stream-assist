use super::ItemView;
use crate::model::RedemptionRef;
use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub enum Origin {
    Chat,
    Reward(RedemptionRef),
    Test,
}
#[derive(Clone, Debug)]
pub struct Item {
    pub id: u64,
    pub origin: Origin,
    pub user: String,
    pub text: String,
}
#[derive(Clone, Copy, Debug)]
pub enum Outcome {
    Played,
    Skipped,
    Rejected,
    QueueFull,
    SynthFailed,
    Cleared,
    Disabled,
    Reconciled,
}
pub fn fulfilled(outcome: &Outcome) -> bool {
    matches!(outcome, Outcome::Played | Outcome::Skipped)
}
#[derive(Default)]
pub struct Queue {
    pending: VecDeque<Item>,
    current: Option<Item>,
}
impl Queue {
    pub fn push(&mut self, item: Item) -> Result<(), Item> {
        if self.pending.len() >= 20 {
            return Err(item);
        }
        self.pending.push_back(item);
        Ok(())
    }
    pub fn take_next(&mut self) -> Option<Item> {
        if self.current.is_some() {
            return None;
        }
        self.current = self.pending.pop_front();
        self.current.clone()
    }
    pub fn current(&self) -> Option<&Item> {
        self.current.as_ref()
    }
    pub fn finish_current(&mut self) -> Option<Item> {
        self.current.take()
    }
    pub fn clear(&mut self) -> Vec<Item> {
        self.pending.drain(..).collect()
    }
    pub fn remove_pending(&mut self, pred: impl Fn(&Item) -> bool) -> Vec<Item> {
        let mut removed = Vec::new();
        self.pending.retain(|item| {
            if pred(item) {
                removed.push(item.clone());
                false
            } else {
                true
            }
        });
        removed
    }
    pub fn view(&self) -> (Option<ItemView>, Vec<ItemView>) {
        let view = |item: &Item| ItemView {
            id: item.id,
            user: item.user.clone(),
            text: item.text.clone(),
            reward: matches!(item.origin, Origin::Reward(_)),
            synthesizing: false,
        };
        (
            self.current.as_ref().map(view),
            self.pending.iter().map(view).collect(),
        )
    }
}
