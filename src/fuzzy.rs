//! Matching for quick open: the query's letters, in order, anywhere in the
//! path. Matches in the file name, at the start of a word and in a row score
//! higher; a shorter path wins a tie.

/// A match: its score (higher is better) and the char positions matched.
#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    pub score: i64,
    pub positions: Vec<usize>,
}

pub fn score(query: &str, path: &str) -> Option<Match> {
    let q: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect();
    if q.is_empty() {
        return Some(Match { score: 0, positions: Vec::new() });
    }
    let chars: Vec<char> = path.chars().collect();
    let lower: Vec<char> = chars.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();
    let name_start = chars.iter().rposition(|&c| c == '/').map_or(0, |i| i + 1);

    // Two passes: from the end, so letters prefer the file name, then forward
    // from the first possible start, keeping the better.
    let best = [greedy_backward(&q, &lower), greedy_forward(&q, &lower)]
        .into_iter()
        .flatten()
        .map(|positions| (rate(&positions, &chars, name_start), positions))
        .max_by_key(|(s, _)| *s)?;
    Some(Match { score: best.0 - chars.len() as i64, positions: best.1 })
}

fn greedy_forward(q: &[char], hay: &[char]) -> Option<Vec<usize>> {
    let mut out = Vec::with_capacity(q.len());
    let mut at = 0;
    for &c in q {
        let found = hay[at..].iter().position(|&h| h == c)? + at;
        out.push(found);
        at = found + 1;
    }
    Some(out)
}

fn greedy_backward(q: &[char], hay: &[char]) -> Option<Vec<usize>> {
    let mut out = Vec::with_capacity(q.len());
    let mut end = hay.len();
    for &c in q.iter().rev() {
        let found = hay[..end].iter().rposition(|&h| h == c)?;
        out.push(found);
        end = found;
    }
    out.reverse();
    Some(out)
}

fn rate(positions: &[usize], chars: &[char], name_start: usize) -> i64 {
    let mut s = 0i64;
    for (i, &p) in positions.iter().enumerate() {
        s += 10;
        if p >= name_start {
            s += 12;
        }
        let word_start = p == 0 || matches!(chars[p - 1], '/' | '-' | '_' | '.' | ' ');
        if word_start {
            s += 15;
        }
        if i > 0 && positions[i - 1] + 1 == p {
            s += 18;
        }
    }
    if positions.first() == Some(&name_start) {
        s += 20;
    }
    s * 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_in_order_anywhere() {
        assert!(score("rdm", "README.md").is_some());
        assert!(score("mdr", "README.md").is_none());
        assert_eq!(score("", "x.md").unwrap().score, 0);
    }

    #[test]
    fn the_file_name_beats_the_folder() {
        let in_name = score("read", "docs/adr/0002-read-only.md").unwrap();
        let readme = score("read", "README.md").unwrap();
        let in_dir = score("read", "reading/notes.md").unwrap();
        assert!(readme.score > in_dir.score);
        assert!(in_name.score > in_dir.score);
        assert_eq!(readme.positions, [0, 1, 2, 3]);
    }

    #[test]
    fn shorter_paths_win_ties() {
        let a = score("todo", "notes/todo.txt").unwrap();
        let b = score("todo", "notes/old/archive/todo.txt").unwrap();
        assert!(a.score > b.score);
    }
}
