use std::cell::Cell;
use std::collections::{HashMap, hash_map::Entry};
use std::hash::Hash;
use std::mem::size_of;
use std::ops::{Deref, DerefMut};

#[cfg(not(test))]
unsafe extern "C" {
    fn cbm_markdown_scratch_change(
        bytes: usize,
        release: bool,
        publish_peak: bool,
    );
}

#[derive(Debug)]
pub(super) struct Exhausted;

pub(super) type ScratchResult<T> = Result<T, Exhausted>;

// Keep capacity charged while its owner is live. Publish the high-water mark
// before a release can cancel the thread-local adds in C's counters.
#[derive(Default)]
pub(super) struct Scratch {
    live: Cell<usize>,
    published_peak: Cell<usize>,
    #[cfg(test)]
    remaining: Cell<Option<usize>>,
}

impl Scratch {
    fn permit(&self) -> ScratchResult<()> {
        #[cfg(test)]
        if let Some(remaining) = self.remaining.get() {
            let next = remaining.checked_sub(1);
            self.remaining.set(next);
            next.ok_or(Exhausted)?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn fail_after(&self, successful_reservations: usize) {
        self.remaining.set(Some(successful_reservations));
    }

    #[cfg(test)]
    pub(super) fn totals(&self) -> (usize, usize) {
        (self.live.get(), self.published_peak.get())
    }
}

struct Charge<'a> {
    scratch: &'a Scratch,
    bytes: usize,
}

impl<'a> Charge<'a> {
    fn new(scratch: &'a Scratch) -> Self {
        Self { scratch, bytes: 0 }
    }

    fn grow(&mut self, bytes: usize) {
        let added = bytes - self.bytes;
        if added != 0 {
            self.bytes = bytes;
            // These live capacities are backed by successful allocations in
            // the same address space, so their total fits in usize.
            self.scratch.live.set(self.scratch.live.get() + added);
            #[cfg(not(test))]
            // SAFETY: The allocation succeeded and is owned on this thread.
            unsafe {
                cbm_markdown_scratch_change(added, false, false)
            };
        }
    }
}

impl Drop for Charge<'_> {
    fn drop(&mut self) {
        if self.bytes == 0 {
            return;
        }
        let live = self.scratch.live.get();
        let publish_peak = live > self.scratch.published_peak.get();
        if publish_peak {
            self.scratch.published_peak.set(live);
        }
        #[cfg(not(test))]
        // SAFETY: This matches the charge applied on this thread.
        unsafe {
            cbm_markdown_scratch_change(self.bytes, true, publish_peak)
        };
        self.scratch.live.set(live - self.bytes);
    }
}

pub(super) struct Buffer<'a, T> {
    data: Vec<T>,
    charge: Charge<'a>,
}

impl<'a, T> Buffer<'a, T> {
    pub(super) fn new(scratch: &'a Scratch) -> Self {
        Self {
            data: Vec::new(),
            charge: Charge::new(scratch),
        }
    }

    pub(super) fn exact(
        scratch: &'a Scratch,
        capacity: usize,
    ) -> ScratchResult<Self> {
        let mut buffer = Self::new(scratch);
        buffer.reserve_exact(capacity)?;
        Ok(buffer)
    }

    pub(super) fn reserve_exact(
        &mut self,
        additional: usize,
    ) -> ScratchResult<()> {
        self.reserve(additional, true)
    }

    fn reserve(&mut self, additional: usize, exact: bool) -> ScratchResult<()> {
        if additional <= self.data.capacity() - self.data.len() {
            return Ok(());
        }
        self.charge.scratch.permit()?;
        if exact {
            self.data.try_reserve_exact(additional)
        } else {
            self.data.try_reserve(additional)
        }
        .map_err(|_| Exhausted)?;
        self.charge.grow(self.data.capacity() * size_of::<T>());
        Ok(())
    }

    pub(super) fn push(&mut self, value: T) -> ScratchResult<()> {
        self.reserve(1, false)?;
        self.data.push(value);
        Ok(())
    }

    pub(super) fn pop(&mut self) -> Option<T> {
        self.data.pop()
    }

    pub(super) fn truncate(&mut self, len: usize) {
        self.data.truncate(len);
    }

    pub(super) fn extend(&mut self, bytes: &[T]) -> ScratchResult<()>
    where
        T: Copy,
    {
        self.reserve(bytes.len(), false)?;
        self.data.extend_from_slice(bytes);
        Ok(())
    }
}

impl<T> Deref for Buffer<'_, T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.data
    }
}

impl<T> DerefMut for Buffer<'_, T> {
    fn deref_mut(&mut self) -> &mut [T] {
        &mut self.data
    }
}

pub(super) struct Map<'a, K, V> {
    data: HashMap<K, V>,
    charge: Charge<'a>,
}

impl<'a, K: Eq + Hash, V> Map<'a, K, V> {
    pub(super) fn new(scratch: &'a Scratch) -> Self {
        Self {
            data: HashMap::new(),
            charge: Charge::new(scratch),
        }
    }

    pub(super) fn entry(&mut self, key: K) -> ScratchResult<Entry<'_, K, V>> {
        if self.data.len() == self.data.capacity() {
            self.charge.scratch.permit()?;
            self.data.try_reserve(1).map_err(|_| Exhausted)?;
            // The standard map exposes usable entry capacity. Control bytes
            // and spare buckets are internal to its allocator.
            self.charge.grow(self.data.capacity() * size_of::<(K, V)>());
        }
        Ok(self.data.entry(key))
    }
}

impl<K, V> Deref for Map<'_, K, V> {
    type Target = HashMap<K, V>;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}
