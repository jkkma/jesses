//! Rebind only exact argv path slots generated for this job. User-added text,
//! quoted descriptions and unrelated path arguments are never globally replaced.
use super::*;

#[derive(Debug)]
struct Token {
    value: String,
    start: usize,
    end: usize,
    operator: bool,
}

fn tokens(text: &str) -> Result<Vec<Token>, AppError> {
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < text.len() {
        let character = text[at..].chars().next().unwrap();
        if character.is_whitespace() {
            at += character.len_utf8();
            continue;
        }
        let start = at;
        if "|&;()<>".contains(character) {
            at += 1;
            if matches!(character, '|' | '&' | '>')
                && text.as_bytes().get(at) == Some(&(character as u8))
            {
                at += 1;
            }
            tokens.push(Token {
                value: text[start..at].to_owned(),
                start,
                end: at,
                operator: true,
            });
            continue;
        }
        let mut value = String::new();
        let mut quote = None;
        while at < text.len() {
            let character = text[at..].chars().next().unwrap();
            if quote == Some('\'') {
                at += character.len_utf8();
                if character == '\'' {
                    quote = None;
                } else {
                    value.push(character);
                }
                continue;
            }
            if character == '"' {
                quote = if quote == Some('"') { None } else { Some('"') };
                at += 1;
                continue;
            }
            if quote.is_none() && character == '\'' {
                quote = Some('\'');
                at += 1;
                continue;
            }
            if character == '\\' {
                let next = text[at + 1..].chars().next();
                if let Some(next) = next
                    && (quote.is_none() || matches!(next, '"' | '\\' | '$' | '`'))
                {
                    value.push(next);
                    at += 1 + next.len_utf8();
                    continue;
                }
            }
            if quote.is_none() && (character.is_whitespace() || "|&;()<>".contains(character)) {
                break;
            }
            value.push(character);
            at += character.len_utf8();
        }
        if quote.is_some() {
            return Err(AppError::new(
                "COMMAND_EDIT_REBIND_REQUIRED",
                "The saved command contains an unfinished quote. Review the command before resuming.",
                None,
            ));
        }
        tokens.push(Token {
            value,
            start,
            end: at,
            operator: false,
        });
    }
    Ok(tokens)
}

fn tool(value: &str) -> Option<&'static str> {
    let name = value.rsplit(['/', '\\']).next()?.to_ascii_lowercase();
    match name.as_str() {
        "av1an" | "av1an.exe" => Some("av1an"),
        "ffmpeg" | "ffmpeg.exe" => Some("ffmpeg"),
        "mkvmerge" | "mkvmerge.exe" => Some("mkvmerge"),
        _ => None,
    }
}

fn slots(tokens: &[Token]) -> Vec<(String, usize)> {
    let mut slots = Vec::new();
    let mut current = None;
    let mut first = true;
    let mut words = Vec::new();
    let finish = |current: Option<&str>, words: &Vec<usize>, slots: &mut Vec<(String, usize)>| {
        if let Some(current) = current {
            if let Some(&index) = words.first() {
                slots.push((format!("{current}:@executable"), index));
            }
            let mut occurrences = std::collections::HashMap::new();
            for pair in words.windows(2) {
                if matches!(
                    tokens[pair[0]].value.as_str(),
                    "-i" | "-o" | "--output" | "--temp" | "--scenes" | "--log-file"
                ) {
                    let occurrence = occurrences
                        .entry(tokens[pair[0]].value.as_str())
                        .or_insert(0);
                    slots.push((
                        format!("{current}:{}#{occurrence}", tokens[pair[0]].value),
                        pair[1],
                    ));
                    *occurrence += 1;
                }
            }
            if current == "ffmpeg"
                && let Some(&last) = words.last()
            {
                slots.push(("ffmpeg:@output".into(), last));
            }
        }
    };
    for (index, token) in tokens.iter().enumerate() {
        if token.operator {
            if matches!(token.value.as_str(), "&&" | "|" | ";" | ")") {
                finish(current, &words, &mut slots);
                current = None;
                words.clear();
                first = true;
            }
            continue;
        }
        if first {
            current = tool(&token.value);
            first = false;
        }
        words.push(index);
    }
    finish(current, &words, &mut slots);
    slots
}

pub(in crate::jobs) fn argument(
    program: &str,
    family: &str,
    flag: &str,
) -> Result<Option<String>, AppError> {
    let tokens = tokens(program)?;
    let mut current = None;
    let mut first = true;
    for (index, token) in tokens.iter().enumerate() {
        if token.operator {
            if matches!(token.value.as_str(), "&&" | "|" | ";" | ")") {
                first = true;
                current = None;
            }
            continue;
        }
        if first {
            current = tool(&token.value);
            first = false;
            continue;
        }
        if current == Some(family) {
            if token.value == flag {
                return Ok(tokens
                    .get(index + 1)
                    .filter(|token| !token.operator)
                    .map(|token| token.value.clone()));
            }
            if let Some(value) = token.value.strip_prefix(&format!("{flag}=")) {
                return Ok(Some(value.to_owned()));
            }
        }
    }
    Ok(None)
}

pub(in crate::jobs) fn words(value: &str) -> Result<Vec<String>, AppError> {
    Ok(tokens(value)?
        .into_iter()
        .map(|token| token.value)
        .collect())
}

fn has_resume(program: &str) -> Result<bool, AppError> {
    let tokens = tokens(program)?;
    let mut current = None;
    let mut first = true;
    for token in &tokens {
        if token.operator {
            if matches!(token.value.as_str(), "&&" | "|" | ";" | ")") {
                first = true;
                current = None;
            }
        } else if first {
            current = tool(&token.value);
            first = false;
        } else if current == Some("av1an") && matches!(token.value.as_str(), "--resume" | "-r") {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(in crate::jobs) fn rebind(
    saved: &EditedCommand,
    generated: &str,
    root: &Path,
    output: &Path,
    id: &str,
    resume: bool,
) -> Result<String, AppError> {
    let old = tokens(&saved.generated)?;
    let new = tokens(generated)?;
    let edited = tokens(&saved.arguments)?;
    let owns = |value: &str| {
        let path = Path::new(value);
        path.is_absolute()
            && (path.starts_with(root)
                || (path.parent() == output.parent()
                    && path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with(&format!(".jesses-{id}-")))))
    };
    let old_slots = slots(&old);
    let new_slots = slots(&new);
    let edited_slots = slots(&edited);
    let mut changes = std::collections::BTreeMap::new();
    for (role, old_index) in old_slots {
        let old_value = &old[old_index].value;
        if !owns(old_value) {
            continue;
        }
        let Some((_, new_index)) = new_slots.iter().find(|(candidate, _)| *candidate == role)
        else {
            continue;
        };
        let new_value = &new[*new_index].value;
        if old_value == new_value {
            continue;
        }
        if !owns(new_value) {
            return Err(AppError::new(
                "COMMAND_EDIT_REBIND_REQUIRED",
                "A generated recovery path moved outside this job's owned locations. Review the command before resuming.",
                None,
            ));
        }
        for (_, edited_index) in edited_slots
            .iter()
            .filter(|(candidate, _)| *candidate == role)
        {
            let token = &edited[*edited_index];
            if token.value == *old_value {
                changes.insert(token.start, (token.end, quote(new_value)));
            }
        }
    }
    if resume
        && !has_resume(&saved.arguments)?
        && let Some((_, index)) = edited_slots
            .iter()
            .find(|(role, _)| role == "av1an:@executable")
    {
        let token = &edited[*index];
        changes.insert(token.end, (token.end, format!(" {}", quote("--resume"))));
    }
    let mut result = saved.arguments.clone();
    for (start, (end, value)) in changes.into_iter().rev() {
        result.replace_range(start..end, &value);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owned_paths_rebind_only_in_matching_generated_argument_roles() {
        let root = if cfg!(windows) {
            "C:/work/job"
        } else {
            "/work/job"
        };
        let output = if cfg!(windows) { "C:/out" } else { "/out" };
        let old = format!(
            "(cd '{root}' && '{root}/launch-1/av1an' '-i' '/media/original.mkv' '-o' '{root}/launch-1/encoded.mkv' '--temp' '{root}/chunks' '--video-params' '--crf 30') && '/usr/bin/ffmpeg' '-i' '{root}/launch-1/encoded.mkv' '{output}/.jesses-id-attempt-1.partial.mkv'"
        );
        let new = old
            .replace("launch-1", "launch-2")
            .replace("attempt-1", "attempt-2");
        let saved = EditedCommand {
            generated: old.clone(),
            arguments: format!(
                "{} && printf '%s' '{root}/launch-1/encoded.mkv' '--resume'",
                old.replace("--crf 30", "--crf 22")
            ),
        };
        let actual = rebind(
            &saved,
            &new,
            Path::new(root),
            &Path::new(output).join("result.mkv"),
            "id",
            true,
        )
        .unwrap();
        assert!(actual.contains(&format!(
            "{} {}",
            quote(&format!("{root}/launch-2/av1an")),
            quote("--resume")
        )));
        assert!(actual.contains(&format!(
            "'-o' {}",
            quote(&format!("{root}/launch-2/encoded.mkv"))
        )));
        assert!(actual.contains(&quote(&format!(
            "{output}/.jesses-id-attempt-2.partial.mkv"
        ))));
        assert!(actual.ends_with(&format!(
            "printf '%s' '{root}/launch-1/encoded.mkv' '--resume'"
        )));
        assert_eq!(
            argument(&actual, "av1an", "--video-params")
                .unwrap()
                .as_deref(),
            Some("--crf 22")
        );
    }
}
