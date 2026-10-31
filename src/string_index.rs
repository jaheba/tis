//! Derived UTF-8 offsets for one immutable string, adapted from Monty PR #1195.
use std::cell::{Cell, RefCell};

const STRIDE: usize = 32;
const SCAN_BUDGET: usize = 32;

#[derive(Debug, Default)]
pub(crate) struct StringIndex {
    length: Cell<Option<usize>>,
    state: RefCell<Option<Box<Cursor>>>,
}
#[derive(Debug, Default)]
struct Cursor {
    last: (usize, usize),
    scanned: usize,
    offsets: Offsets,
}
#[derive(Debug, Default)]
enum Offsets {
    #[default]
    Empty,
    Compact(Box<[u32]>),
    Wide(Box<[usize]>),
}
impl Offsets {
    fn len(&self) -> usize {
        match self {
            Self::Empty => 0,
            Self::Compact(v) => v.len(),
            Self::Wide(v) => v.len(),
        }
    }
    fn get(&self, i: usize) -> usize {
        match self {
            Self::Empty => 0,
            Self::Compact(v) => v[i] as usize,
            Self::Wide(v) => v[i],
        }
    }
    fn anchor(&self, byte: usize) -> (usize, usize) {
        let (mut low, mut high) = (0, self.len());
        while low < high {
            let mid = low + (high - low) / 2;
            if self.get(mid) <= byte {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        let index = low.saturating_sub(1);
        (index * STRIDE, self.get(index))
    }
}
impl StringIndex {
    pub fn with_length(length: Option<usize>) -> Self {
        Self {
            length: Cell::new(length),
            state: RefCell::new(None),
        }
    }
    pub fn len(&self, text: &str) -> usize {
        if let Some(length) = self.length.get() {
            return length;
        }
        let length = text.chars().count();
        self.length.set(Some(length));
        length
    }
    pub fn character(&self, text: &str, index: i64) -> Option<char> {
        // Endpoints do not need a length scan or cursor allocation.
        if index == 0 {
            return text.chars().next();
        }
        if index == -1 {
            return text.chars().next_back();
        }
        let target = if index < 0 {
            self.len(text)
                .checked_sub(usize::try_from(index.unsigned_abs()).ok()?)?
        } else {
            usize::try_from(index).ok()?
        };
        text[self.byte_offset(text, target)..].chars().next()
    }
    pub fn byte_offset(&self, text: &str, target: usize) -> usize {
        if target == 0 {
            return 0;
        }
        if self.length.get().is_some_and(|length| target >= length) {
            return text.len();
        }
        let mut cursor = self.state.borrow_mut();
        let cursor = cursor.get_or_insert_with(Box::default);
        let checkpoint = target / STRIDE;
        let (mut character, mut byte) = if checkpoint < cursor.offsets.len() {
            (checkpoint * STRIDE, cursor.offsets.get(checkpoint))
        } else {
            (0, 0)
        };
        let (last_character, last_byte) = cursor.last;
        if last_character <= target && last_character >= character {
            (character, byte) = cursor.last;
        } else if last_character > target && last_character - target <= 8 {
            let mut chars = text[..last_byte].chars();
            chars.nth_back(last_character - target - 1);
            let result = chars.as_str().len();
            cursor.last = (target, result);
            return result;
        }
        if target == character {
            return byte;
        }
        let mut chars = text[byte..].chars();
        if chars.nth(target - character - 1).is_none() {
            self.len(text);
            return text.len();
        }
        let result = text.len() - chars.as_str().len();
        cursor.last = (target, result);
        self.charge(text, cursor, target - character, result - byte);
        result
    }
    pub fn char_offset(&self, text: &str, target: usize) -> usize {
        if target == 0 {
            return 0;
        }
        if target == text.len() {
            return self.len(text);
        }
        let mut cursor = self.state.borrow_mut();
        let cursor = cursor.get_or_insert_with(Box::default);
        let (mut character, mut byte) = cursor.offsets.anchor(target);
        if cursor.last.1 <= target && cursor.last.1 >= byte {
            (character, byte) = cursor.last;
        }
        let traversed = text[byte..target].chars().count();
        let result = character + traversed;
        cursor.last = (result, target);
        self.charge(text, cursor, traversed, target - byte);
        result
    }
    fn charge(&self, text: &str, cursor: &mut Cursor, characters: usize, bytes: usize) {
        if cursor.offsets.len() != 0 || characters <= 8 {
            return;
        }
        cursor.scanned = cursor.scanned.saturating_add(bytes);
        if cursor.scanned < text.len().saturating_mul(SCAN_BUDGET) {
            return;
        }
        let count = self.len(text).div_ceil(STRIDE);
        if u32::try_from(text.len()).is_ok() {
            let mut offsets = Vec::with_capacity(count);
            offsets.extend(
                text.char_indices()
                    .step_by(STRIDE)
                    .map(|(byte, _)| byte as u32),
            );
            cursor.offsets = Offsets::Compact(offsets.into_boxed_slice());
        } else {
            let mut offsets = Vec::with_capacity(count);
            offsets.extend(text.char_indices().step_by(STRIDE).map(|(byte, _)| byte));
            cursor.offsets = Offsets::Wide(offsets.into_boxed_slice());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn offsets_match_characters_after_random_access_and_reverse_local_walks() {
        let text = "aé中🐍".repeat(1000);
        let chars: Vec<_> = text.chars().collect();
        let boundaries: Vec<_> = text
            .char_indices()
            .map(|(i, _)| i)
            .chain([text.len()])
            .collect();
        let cache = StringIndex::default();
        for i in 0..5000 {
            let index = i * 7919 % (chars.len() + 1);
            assert_eq!(cache.byte_offset(&text, index), boundaries[index]);
            assert_eq!(cache.char_offset(&text, boundaries[index]), index);
            assert_eq!(
                cache.character(&text, index as i64),
                chars.get(index).copied()
            );
        }
        assert!(cache.state.borrow().as_ref().unwrap().offsets.len() > 0);
        for index in (0..chars.len()).rev() {
            assert_eq!(cache.character(&text, index as i64), Some(chars[index]));
        }
        assert_eq!(cache.character(&text, i64::MIN), None);
        assert_eq!(cache.character(&text, i64::MAX), None);
    }
    #[test]
    fn endpoints_and_sequential_access_do_not_build_checkpoints() {
        let text = "aé中🐍".repeat(1000);
        let cache = StringIndex::default();
        for _ in 0..100 {
            assert_eq!(cache.character(&text, 0), Some('a'));
            assert_eq!(cache.character(&text, -1), Some('🐍'));
        }
        assert!(cache.state.borrow().is_none());
        for i in 0..4000 {
            cache.character(&text, i);
        }
        assert_eq!(cache.state.borrow().as_ref().unwrap().offsets.len(), 0);
    }
}
