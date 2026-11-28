//! File-local Ruby constants and method ancestry. The native walker supplies
//! syntax and emits call rows; the registry supplies exact method identities.

use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};

#[derive(Clone, Copy)]
enum MixinKind {
    Include,
    Prepend,
    Extend,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Side {
    Instance,
    Singleton,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Search {
    Receiver,
    SelfDispatch,
    Super,
}

struct Reference {
    path: CString,
    nesting: Vec<u8>,
    target: Option<CString>,
}

impl Reference {
    fn new(nesting: &[u8], path: &CStr) -> Self {
        Self {
            path: path.into(),
            nesting: nesting.into(),
            target: None,
        }
    }
}

#[derive(Default)]
struct Class {
    parent: Option<Reference>,
    includes: Vec<Reference>,
    prepends: Vec<Reference>,
    extends: Vec<Reference>,
}

#[derive(Default)]
struct Ancestry {
    classes: HashMap<CString, Class>,
    // Constant paths are distinct from extractor QNs. Compact and nested
    // declarations can have the same path, and the first declaration wins.
    constants: HashMap<Vec<u8>, CString>,
}

fn qualified_path(nesting: &[u8], path: &CStr) -> Vec<u8> {
    let mut dotted =
        Vec::with_capacity(nesting.len() + 1 + path.to_bytes().len());
    let mut rest = nesting;
    while let Some((&byte, tail)) = rest.split_first() {
        if byte == b':' && tail.starts_with(b":") {
            dotted.push(b'.');
            rest = &tail[1..];
        } else {
            dotted.push(byte);
            rest = tail;
        }
    }
    if !dotted.is_empty() {
        dotted.push(b'.');
    }
    dotted.extend(path.to_bytes());
    dotted
}

fn constant_at<'a>(
    constants: &'a HashMap<Vec<u8>, CString>,
    mut nesting: &[u8],
    path: &CStr,
) -> Option<&'a CStr> {
    if path.is_empty() {
        return None;
    }
    // Preserve the native bound on lexical probing. Drop raw frames before
    // normalizing "::" so class A::B never opens a separate A frame.
    if nesting.len() < 512 {
        while !nesting.is_empty() {
            if let Some(qn) = constants.get(&qualified_path(nesting, path)) {
                return Some(qn);
            }
            nesting = nesting
                .iter()
                .rposition(|&byte| byte == b'.')
                .map_or(&[], |dot| &nesting[..dot]);
        }
    }
    constants.get(path.to_bytes()).map(CString::as_c_str)
}

impl Ancestry {
    fn declare(
        &mut self,
        qn: &CStr,
        nesting: &[u8],
        path: &CStr,
        parent: Option<&CStr>,
    ) {
        let class = self.classes.entry(qn.into()).or_insert_with(|| {
            self.constants
                .entry(qualified_path(nesting, path))
                .or_insert_with(|| qn.into());
            Class::default()
        });
        if class.parent.is_none() {
            class.parent = parent.map(|path| Reference::new(nesting, path));
        }
    }

    fn mixin(
        &mut self,
        owner: &CStr,
        kind: MixinKind,
        nesting: &[u8],
        path: &CStr,
    ) {
        if let Some(class) = self.classes.get_mut(owner) {
            let refs = match kind {
                MixinKind::Include => &mut class.includes,
                MixinKind::Prepend => &mut class.prepends,
                MixinKind::Extend => &mut class.extends,
            };
            refs.push(Reference::new(nesting, path));
        }
    }

    fn finalize(&mut self, mut builtin: impl FnMut(&CStr) -> Option<CString>) {
        for class in self.classes.values_mut() {
            for reference in class
                .parent
                .iter_mut()
                .chain(&mut class.includes)
                .chain(&mut class.prepends)
                .chain(&mut class.extends)
            {
                reference.target = constant_at(
                    &self.constants,
                    &reference.nesting,
                    &reference.path,
                )
                .map(CString::from)
                .or_else(|| builtin(&reference.path));
            }
        }
    }

    /// An error means the class explicitly names an unresolved superclass.
    fn parent(&self, qn: &CStr) -> Result<Option<&CStr>, ()> {
        if let Some(class) = self.classes.get(qn) {
            return class
                .parent
                .as_ref()
                .map(|reference| reference.target.as_deref().ok_or(()))
                .transpose();
        }
        // Rails conventions for types outside the file-local class table.
        Ok(match qn.to_bytes() {
            b"ApplicationRecord" => Some(c"ActiveRecord.Base"),
            b"ApplicationController" => Some(c"ActionController.Base"),
            b"ApplicationJob" => Some(c"ActiveJob.Base"),
            b"ApplicationMailer" => Some(c"ActionMailer.Base"),
            _ => None,
        })
    }

    fn is_ar_model<'a>(&'a self, mut qn: &'a CStr, max_depth: u32) -> bool {
        fn model_base(qn: &CStr) -> bool {
            matches!(qn.to_bytes(), b"ActiveRecord.Base" | b"ApplicationRecord")
        }

        for _ in 0..=max_depth {
            if model_base(qn)
                || self.classes.get(qn).is_some_and(|class| {
                    class.parent.as_ref().is_some_and(|r| model_base(&r.path))
                })
            {
                return true;
            }
            match self.parent(qn) {
                Ok(Some(parent)) if parent != qn => qn = parent,
                _ => break,
            }
        }
        false
    }

    fn lookup<T>(
        &self,
        qn: &CStr,
        side: Side,
        search: Search,
        max_depth: u32,
        probe: impl FnMut(&CStr) -> Option<T>,
    ) -> (Lookup<T>, u64) {
        let mut walk = Walk {
            ancestry: self,
            max_depth,
            probe,
            seen: HashSet::new(),
            steps: 0,
        };
        let result = match search {
            Search::Super => walk.super_method(qn, side),
            _ => walk.method(qn, side, 0),
        };
        let result = match result {
            Lookup::Miss
                if search == Search::Receiver
                    && side == Side::Instance
                    && qn != c"Object" =>
            {
                // Universal receiver typing after a clean miss.
                Lookup::from_option((walk.probe)(c"Object"))
            }
            result => result,
        };
        (result, walk.steps)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Lookup<T> {
    Hit(T),
    Miss,
    Blocked,
}

impl<T> Lookup<T> {
    fn from_option(target: Option<T>) -> Self {
        target.map_or(Self::Miss, Self::Hit)
    }

    fn or_else(self, next: impl FnOnce() -> Self) -> Self {
        match self {
            Self::Miss => next(),
            result => result,
        }
    }
}

struct Walk<'a, F> {
    ancestry: &'a Ancestry,
    max_depth: u32,
    probe: F,
    seen: HashSet<(Side, &'a CStr)>,
    steps: u64,
}

impl<'a, T, F: FnMut(&CStr) -> Option<T>> Walk<'a, F> {
    fn method(&mut self, qn: &'a CStr, side: Side, depth: u32) -> Lookup<T> {
        if depth > self.max_depth {
            return Lookup::Miss;
        }
        self.steps += 1;
        let class = self.ancestry.classes.get(qn);
        if class.is_some() && !self.seen.insert((side, qn)) {
            return Lookup::Miss;
        }
        let found = match side {
            Side::Instance => self
                .mixins(class.map(|c| c.prepends.as_slice()), depth)
                .or_else(|| Lookup::from_option((self.probe)(qn)))
                .or_else(|| {
                    self.mixins(class.map(|c| c.includes.as_slice()), depth)
                }),
            Side::Singleton => {
                let mut key = qn.to_bytes().to_vec();
                key.extend(b".self\0");
                let key = CStr::from_bytes_with_nul(&key)
                    .expect("singleton receiver key");
                Lookup::from_option((self.probe)(key)).or_else(|| {
                    self.mixins(class.map(|c| c.extends.as_slice()), depth)
                })
            }
        };
        found.or_else(|| match self.ancestry.parent(qn) {
            Ok(Some(parent)) if parent != qn => {
                self.method(parent, side, depth + 1)
            }
            Ok(_) => Lookup::Miss,
            Err(()) => Lookup::Blocked,
        })
    }

    fn mixins(
        &mut self,
        refs: Option<&'a [Reference]>,
        depth: u32,
    ) -> Lookup<T> {
        for reference in refs.unwrap_or_default().iter().rev() {
            let Some(module) = reference.target.as_deref() else {
                return Lookup::Blocked;
            };
            match self.method(module, Side::Instance, depth + 1) {
                Lookup::Miss => {}
                result => return result,
            }
        }
        Lookup::Miss
    }

    fn super_method(&mut self, qn: &'a CStr, side: Side) -> Lookup<T> {
        self.mixins(
            self.ancestry.classes.get(qn).map(|class| match side {
                Side::Instance => class.includes.as_slice(),
                Side::Singleton => class.extends.as_slice(),
            }),
            0,
        )
        .or_else(|| match self.ancestry.parent(qn) {
            Ok(Some(parent)) => self.method(parent, side, 0),
            // The native super entry point stops at an unresolved direct
            // parent without starting an ancestry walk.
            Ok(None) | Err(()) => Lookup::Miss,
        })
    }
}

#[cfg(feature = "native-engine-abi")]
#[path = "ruby_lookup_abi.rs"]
mod abi;

#[cfg(test)]
#[path = "ruby_lookup_tests.rs"]
mod tests;
