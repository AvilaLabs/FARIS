//! Work done one unit per frame. The browser page has no threads, so a list
//! of long calculations is run one item at a time between frames: the page
//! keeps painting and answering input while it works.

use std::collections::VecDeque;

pub struct Steps<I, O> {
    queue: VecDeque<I>,
    done: Vec<O>,
    work: Box<dyn FnMut(I) -> Result<O, String> + Send>,
}

impl<I, O> Steps<I, O> {
    pub fn new(items: Vec<I>, work: impl FnMut(I) -> Result<O, String> + Send + 'static) -> Self {
        Self {
            queue: items.into(),
            done: Vec::new(),
            work: Box::new(work),
        }
    }

    /// Run the next unit. `Some` once everything is done, or at the first failure.
    pub fn advance(&mut self) -> Option<Result<Vec<O>, String>> {
        let Some(item) = self.queue.pop_front() else {
            return Some(Ok(std::mem::take(&mut self.done)));
        };
        match (self.work)(item) {
            Ok(output) => {
                self.done.push(output);
                self.queue
                    .is_empty()
                    .then(|| Ok(std::mem::take(&mut self.done)))
            }
            Err(error) => Some(Err(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_item_per_advance_and_the_results_in_order() {
        let mut steps = Steps::new(vec![1, 2, 3], |n| Ok(n * 10));
        assert!(steps.advance().is_none());
        assert!(steps.advance().is_none());
        assert_eq!(steps.advance().unwrap().unwrap(), vec![10, 20, 30]);
    }

    #[test]
    fn a_failure_ends_the_run_with_its_message() {
        let mut steps = Steps::new(vec![1, 2], |n| {
            if n == 2 { Err("no".to_string()) } else { Ok(n) }
        });
        assert!(steps.advance().is_none());
        assert_eq!(steps.advance().unwrap().unwrap_err(), "no");
    }

    #[test]
    fn an_empty_list_finishes_at_once() {
        let mut steps: Steps<u8, u8> = Steps::new(vec![], Ok);
        assert!(steps.advance().unwrap().unwrap().is_empty());
    }
}
