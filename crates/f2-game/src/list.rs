//! Intrusive doubly-linked lists with a sentinel head, mirroring the C pattern
//!
//! ```c
//! for (e = list.head.next; e != &list.head; e = e->next) ...
//! ```
//!
//! Elements live in an array/arena; each element embeds a [`Links`]. `None`
//! stands for the sentinel head, so iteration order and insertion/removal
//! semantics match the original exactly (insertion at head, removal while
//! iterating by grabbing `next` first, ...).

use crate::prelude::*;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Links<I> {
    pub next: Option<I>,
    pub prev: Option<I>,
}

/// List head (the sentinel's own links): `first` = `head.next`, `last` = `head.prev`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Head<I> {
    pub first: Option<I>,
    pub last: Option<I>,
    pub count: i32,
}

impl<I> Default for Head<I> {
    fn default() -> Self {
        Head { first: None, last: None, count: 0 }
    }
}

/// Access to the embedded links of elements stored in some container.
pub trait LinkStore<I: Copy + PartialEq> {
    fn links(&self, id: I) -> &Links<I>;
    fn links_mut(&mut self, id: I) -> &mut Links<I>;
}

impl<I: Copy + PartialEq> Head<I> {
    /// Inserts after the sentinel (the C `AddXxx` functions insert at the head).
    pub fn push_front<S: LinkStore<I>>(&mut self, s: &mut S, id: I) {
        let old_first = self.first;
        {
            let l = s.links_mut(id);
            l.prev = None;
            l.next = old_first;
        }
        match old_first {
            Some(f) => s.links_mut(f).prev = Some(id),
            None => self.last = Some(id),
        }
        self.first = Some(id);
        self.count += 1;
    }

    /// Inserts before the sentinel (end of list).
    pub fn push_back<S: LinkStore<I>>(&mut self, s: &mut S, id: I) {
        let old_last = self.last;
        {
            let l = s.links_mut(id);
            l.next = None;
            l.prev = old_last;
        }
        match old_last {
            Some(p) => s.links_mut(p).next = Some(id),
            None => self.first = Some(id),
        }
        self.last = Some(id);
        self.count += 1;
    }

    /// Inserts `id` immediately before the listed element `at`
    /// (`AddPlatform`'s "add before existing one of the same kind").
    pub fn insert_before<S: LinkStore<I>>(&mut self, s: &mut S, at: I, id: I) {
        let prev = s.links(at).prev;
        {
            let l = s.links_mut(id);
            l.prev = prev;
            l.next = Some(at);
        }
        match prev {
            Some(p) => s.links_mut(p).next = Some(id),
            None => self.first = Some(id),
        }
        s.links_mut(at).prev = Some(id);
        self.count += 1;
    }

    /// Unlinks `id` (C `SubXxx`). The element's own links are left untouched,
    /// like the original, so an iterator holding `next` keeps working.
    pub fn remove<S: LinkStore<I>>(&mut self, s: &mut S, id: I) {
        let Links { next, prev } = *s.links(id);
        match prev {
            Some(p) => s.links_mut(p).next = next,
            None => self.first = next,
        }
        match next {
            Some(n) => s.links_mut(n).prev = prev,
            None => self.last = prev,
        }
        self.count -= 1;
    }

    /// Snapshot of the current order (use when the body may mutate the list).
    pub fn ids<S: LinkStore<I>>(&self, s: &S) -> Vec<I> {
        let mut v = Vec::with_capacity(self.count.max(0) as usize);
        let mut c = self.first;
        while let Some(id) = c {
            v.push(id);
            c = s.links(id).next;
        }
        v
    }
}

impl<I: Copy + PartialEq, T: HasLinks<I>> LinkStore<I> for Vec<T>
where
    I: Into<usize>,
{
    fn links(&self, id: I) -> &Links<I> {
        self[id.into()].links()
    }
    fn links_mut(&mut self, id: I) -> &mut Links<I> {
        self[id.into()].links_mut()
    }
}

/// Elements that embed their list links.
pub trait HasLinks<I> {
    fn links(&self) -> &Links<I>;
    fn links_mut(&mut self) -> &mut Links<I>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Ix(usize);
    impl From<Ix> for usize {
        fn from(i: Ix) -> usize {
            i.0
        }
    }
    struct E {
        l: Links<Ix>,
    }
    impl HasLinks<Ix> for E {
        fn links(&self) -> &Links<Ix> {
            &self.l
        }
        fn links_mut(&mut self) -> &mut Links<Ix> {
            &mut self.l
        }
    }

    #[test]
    fn order_and_removal() {
        let mut v: Vec<E> = (0..4).map(|_| E { l: Links { next: None, prev: None } }).collect();
        let mut h = Head::default();
        for i in 0..3 {
            h.push_front(&mut v, Ix(i));
        }
        h.push_back(&mut v, Ix(3));
        assert_eq!(h.ids(&v), vec![Ix(2), Ix(1), Ix(0), Ix(3)]);
        h.remove(&mut v, Ix(1));
        assert_eq!(h.ids(&v), vec![Ix(2), Ix(0), Ix(3)]);
        assert_eq!(h.count, 3);
        h.remove(&mut v, Ix(2));
        h.remove(&mut v, Ix(3));
        assert_eq!(h.ids(&v), vec![Ix(0)]);
        assert_eq!((h.first, h.last), (Some(Ix(0)), Some(Ix(0))));
    }
}
