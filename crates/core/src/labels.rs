use std::collections::BTreeMap;

pub fn project_label(raw_tab_name: &str, stable_tab_id: u32) -> String {
    let metadata = decode_super_tabs_name(raw_tab_name).unwrap_or_default();
    let directory = metadata
        .get("directory")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());
    let worktree = metadata
        .get("worktree")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());
    match (directory, worktree) {
        (Some(directory), Some(worktree)) if directory != worktree => {
            format!("{directory} · {worktree}")
        }
        (Some(directory), _) => directory.to_owned(),
        (_, Some(worktree)) => worktree.to_owned(),
        _ if !raw_tab_name.trim().is_empty() && !metadata.contains_key("__super_tabs_id") => {
            raw_tab_name.trim().to_owned()
        }
        _ => format!("Zellij tab {stable_tab_id}"),
    }
}

pub fn decode_super_tabs_name(input: &str) -> Option<BTreeMap<String, String>> {
    let mut result = BTreeMap::new();
    let mut rest = input.trim();
    while !rest.is_empty() {
        let (key, after_equals) = rest.split_once('=')?;
        let key = key.trim();
        if key.is_empty() || key.contains('|') {
            return None;
        }
        let (value, remaining) = parse_quoted(after_equals.trim_start())?;
        result.insert(key.to_owned(), value);
        rest = remaining.trim_start();
        if rest.is_empty() {
            break;
        }
        rest = rest.strip_prefix('|')?.trim_start();
    }
    Some(result)
}

fn parse_quoted(input: &str) -> Option<(String, &str)> {
    let mut chars = input.char_indices();
    if chars.next()?.1 != '"' {
        return None;
    }
    let mut value = String::new();
    let mut escaped = false;
    for (index, ch) in chars {
        if escaped {
            match ch {
                '\\' | '"' => value.push(ch),
                _ => return None,
            }
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Some((value, &input[index + ch.len_utf8()..]));
        } else {
            value.push(ch);
        }
    }
    None
}
