use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct Ring<T> {
    cap: usize,
    inner: VecDeque<T>,
}

impl<T: Clone> Ring<T> {
    pub fn new(cap: usize) -> Self {
        Self {
            cap,
            inner: VecDeque::with_capacity(cap),
        }
    }

    pub fn push(&mut self, value: T) {
        if self.inner.len() == self.cap {
            self.inner.pop_front();
        }
        self.inner.push_back(value);
    }

    pub fn last(&self) -> Option<&T> {
        self.inner.back()
    }

    pub fn to_vec(&self) -> Vec<T> {
        self.inner.iter().cloned().collect()
    }
}
