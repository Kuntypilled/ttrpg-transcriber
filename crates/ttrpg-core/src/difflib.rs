//! `difflib.SequenceMatcher` from Python's standard library, as far as the alias
//! report uses it: `ratio`, `quick_ratio` and `real_quick_ratio` on two strings.
//! It is the Ratcliff/Obershelp measure, not Levenshtein;
//! the report's 0.70 threshold was tuned against it, so it is ported exactly,
//! including how ties between equally long matches are broken and the "popular
//! element" rule for a second string of 200 characters or more.
//! `tests/difflib.rs` checks it against Python on a fixed set of pairs.

use std::collections::HashMap;

pub struct SequenceMatcher {
    a: Vec<char>,
    b: Vec<char>,
}

fn calculate_ratio(matches: usize, length: usize) -> f64 {
    if length > 0 {
        2.0 * matches as f64 / length as f64
    } else {
        1.0
    }
}

impl SequenceMatcher {
    pub fn new(a: &str, b: &str) -> Self {
        Self {
            a: a.chars().collect(),
            b: b.chars().collect(),
        }
    }

    /// An upper bound on `ratio`, from the lengths alone.
    pub fn real_quick_ratio(&self) -> f64 {
        let (la, lb) = (self.a.len(), self.b.len());
        calculate_ratio(la.min(lb), la + lb)
    }

    /// An upper bound on `ratio`, from the letters in common.
    pub fn quick_ratio(&self) -> f64 {
        let mut avail: HashMap<char, i64> = HashMap::new();
        let mut full: HashMap<char, i64> = HashMap::new();
        for c in &self.b {
            *full.entry(*c).or_insert(0) += 1;
        }
        let mut matches = 0;
        for c in &self.a {
            let numb = *avail.entry(*c).or_insert_with(|| full.get(c).copied().unwrap_or(0));
            avail.insert(*c, numb - 1);
            if numb > 0 {
                matches += 1;
            }
        }
        calculate_ratio(matches, self.a.len() + self.b.len())
    }

    /// `2 * M / T`: M the total size of the matching blocks, T the length of both strings.
    pub fn ratio(&self) -> f64 {
        let matches: usize = self.matching_blocks().iter().map(|m| m.2).sum();
        calculate_ratio(matches, self.a.len() + self.b.len())
    }

    /// Positions of each character of `b`, without the popular ones (`autojunk`).
    fn b2j(&self) -> HashMap<char, Vec<usize>> {
        let mut b2j: HashMap<char, Vec<usize>> = HashMap::new();
        for (i, c) in self.b.iter().enumerate() {
            b2j.entry(*c).or_default().push(i);
        }
        let n = self.b.len();
        if n >= 200 {
            let ntest = n / 100 + 1;
            b2j.retain(|_, idxs| idxs.len() <= ntest);
        }
        b2j
    }

    /// `(i, j, k)` with `a[i..i+k] == b[j..j+k]`: the longest block inside the given
    /// ranges, the earliest in `a`, then the earliest in `b`.
    fn find_longest_match(
        &self,
        b2j: &HashMap<char, Vec<usize>>,
        (alo, ahi, blo, bhi): (usize, usize, usize, usize),
    ) -> (usize, usize, usize) {
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0usize);
        let mut j2len: HashMap<usize, usize> = HashMap::new();
        for i in alo..ahi {
            let mut newj2len: HashMap<usize, usize> = HashMap::new();
            if let Some(js) = b2j.get(&self.a[i]) {
                for &j in js {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let k = if j > 0 {
                        j2len.get(&(j - 1)).copied().unwrap_or(0)
                    } else {
                        0
                    } + 1;
                    newj2len.insert(j, k);
                    if k > bestsize {
                        (besti, bestj, bestsize) = (i + 1 - k, j + 1 - k, k);
                    }
                }
            }
            j2len = newj2len;
        }
        // Extend over the elements that were left out as popular (there is no junk).
        while besti > alo && bestj > blo && self.a[besti - 1] == self.b[bestj - 1] {
            (besti, bestj, bestsize) = (besti - 1, bestj - 1, bestsize + 1);
        }
        while besti + bestsize < ahi && bestj + bestsize < bhi && self.a[besti + bestsize] == self.b[bestj + bestsize] {
            bestsize += 1;
        }
        (besti, bestj, bestsize)
    }

    fn matching_blocks(&self) -> Vec<(usize, usize, usize)> {
        let b2j = self.b2j();
        let mut queue = vec![(0usize, self.a.len(), 0usize, self.b.len())];
        let mut blocks = Vec::new();
        while let Some((alo, ahi, blo, bhi)) = queue.pop() {
            let (i, j, k) = self.find_longest_match(&b2j, (alo, ahi, blo, bhi));
            if k > 0 {
                blocks.push((i, j, k));
                if alo < i && blo < j {
                    queue.push((alo, i, blo, j));
                }
                if i + k < ahi && j + k < bhi {
                    queue.push((i + k, ahi, j + k, bhi));
                }
            }
        }
        blocks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(a: &str, b: &str) -> f64 {
        SequenceMatcher::new(a, b).ratio()
    }

    #[test]
    fn known_values_from_the_python_documentation_and_the_report() {
        assert_eq!(r("abcd", "bcde"), 0.75);
        assert_eq!(r("", ""), 1.0);
        assert_eq!(r("abc", ""), 0.0);
        assert_eq!(r("vashti", "vashti"), 1.0);
        // Longest block first, then the leftovers on each side: "ab" + "e".
        assert!((r("abxcde", "abycde") - 10.0 / 12.0).abs() < 1e-12);
        let sm = SequenceMatcher::new("vashti", "vashtee");
        assert!(sm.real_quick_ratio() >= sm.quick_ratio() && sm.quick_ratio() >= sm.ratio());
    }
}
