//! Single-pass, lossless parser for `~/.ssh/config`.
//!
//! Produces a [`SshConfig`] such that `render(parse(s)) == s` byte-for-byte for
//! any unedited input. See [`super::writer`] for the rendering side.

use std::path::PathBuf;

use super::model::{
    BodyLine, HostBlock, IncludeLine, Item, MatchBlock, Newline, OptionLine, RawLine, SshConfig,
};
use super::tokens::tokenize_args;

/// Classification of a single physical line (without its line ending).
enum Classified {
    Blank,
    Comment,
    Option(OptionLine),
}

/// Parse the textual content loaded from `path` into a lossless model.
pub fn parse(path: PathBuf, content: &str) -> SshConfig {
    // Strip a leading UTF-8 BOM (do not re-add on write).
    let (content, had_bom) = match content.strip_prefix('\u{feff}') {
        Some(rest) => (rest, true),
        None => (content, false),
    };

    if content.is_empty() {
        return SshConfig {
            path,
            had_bom,
            ..Default::default()
        };
    }

    let newline = if content.contains("\r\n") {
        Newline::Crlf
    } else {
        Newline::Lf
    };
    let trailing_newline = content.ends_with('\n');

    // Split into physical lines without endings.
    let mut raw_lines: Vec<&str> = content.split('\n').collect();
    if trailing_newline {
        // Drop the empty artifact produced by the final '\n'.
        raw_lines.pop();
    }

    let mut items: Vec<Item> = Vec::new();
    // The block currently being filled (its body receives subsequent lines).
    let mut cur: Option<Item> = None;

    for (i, raw) in raw_lines.iter().enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let src = Some(i + 1);

        match classify(line, src) {
            Classified::Option(opt) if opt.is("Host") => {
                let pre = steal_trailing_comments(&mut cur, &mut items);
                close_block(&mut cur, &mut items);
                let patterns = tokenize_args(&opt.args);
                cur = Some(Item::Host(HostBlock {
                    pre,
                    header: opt,
                    patterns,
                    body: Vec::new(),
                }));
            }
            Classified::Option(opt) if opt.is("Match") => {
                let pre = steal_trailing_comments(&mut cur, &mut items);
                close_block(&mut cur, &mut items);
                let criteria_raw = opt.args.clone();
                cur = Some(Item::Match(MatchBlock {
                    pre,
                    header: opt,
                    criteria_raw,
                    body: Vec::new(),
                }));
            }
            Classified::Option(opt) if opt.is("Include") && cur.is_none() => {
                let paths = tokenize_args(&opt.args);
                items.push(Item::Include(IncludeLine { option: opt, paths }));
            }
            Classified::Option(opt) => push_option(&mut cur, &mut items, opt),
            Classified::Comment => push_raw(
                &mut cur,
                &mut items,
                RawLine {
                    text: line.to_string(),
                    src_line: src,
                },
                true,
            ),
            Classified::Blank => push_raw(
                &mut cur,
                &mut items,
                RawLine {
                    text: line.to_string(),
                    src_line: src,
                },
                false,
            ),
        }
    }
    close_block(&mut cur, &mut items);

    SshConfig {
        items,
        newline,
        trailing_newline,
        path,
        dirty: false,
        had_bom,
        bak_done: false,
    }
}

/// Split a physical line into indent / keyword / sep / args, preserving bytes.
fn classify(line: &str, src: Option<usize>) -> Classified {
    if line.trim().is_empty() {
        return Classified::Blank;
    }
    let trimmed_start = line.trim_start();
    if trimmed_start.starts_with('#') {
        return Classified::Comment;
    }

    let indent_len = line.len() - trimmed_start.len();
    let indent = line[..indent_len].to_string();
    let rest = &line[indent_len..];

    // Keyword runs until the first whitespace or '='.
    let kw_end = rest
        .find(|c: char| c.is_whitespace() || c == '=')
        .unwrap_or(rest.len());
    let keyword = rest[..kw_end].to_string();
    let after_kw = &rest[kw_end..];

    // Separator is the leading run of whitespace / '=' characters.
    let sep_end = after_kw
        .find(|c: char| !(c.is_whitespace() || c == '='))
        .unwrap_or(after_kw.len());
    let sep = after_kw[..sep_end].to_string();
    let args = after_kw[sep_end..].to_string();

    Classified::Option(OptionLine {
        indent,
        keyword,
        sep,
        args,
        src_line: src,
    })
}

/// Append an option line to the current block body, or to top level as Global.
fn push_option(cur: &mut Option<Item>, items: &mut Vec<Item>, opt: OptionLine) {
    match cur {
        Some(Item::Host(b)) => b.body.push(BodyLine::Option(opt)),
        Some(Item::Match(b)) => b.body.push(BodyLine::Option(opt)),
        _ => items.push(Item::Global(opt)),
    }
}

/// Append a comment/blank line to the current block body, or to top level.
fn push_raw(cur: &mut Option<Item>, items: &mut Vec<Item>, raw: RawLine, is_comment: bool) {
    match cur {
        Some(Item::Host(b)) => b.body.push(if is_comment {
            BodyLine::Comment(raw)
        } else {
            BodyLine::Blank(raw)
        }),
        Some(Item::Match(b)) => b.body.push(if is_comment {
            BodyLine::Comment(raw)
        } else {
            BodyLine::Blank(raw)
        }),
        _ => items.push(if is_comment {
            Item::Comment(raw)
        } else {
            Item::Blank(raw)
        }),
    }
}

/// Commit the current block into the items list.
fn flush(cur: &mut Option<Item>, items: &mut Vec<Item>) {
    if let Some(item) = cur.take() {
        items.push(item);
    }
}

/// Close the current block: float its trailing blank lines up to the top level
/// (they are separators between blocks, not part of the block), then commit it.
/// The floated blanks are pushed *after* the block, in original order.
fn close_block(cur: &mut Option<Item>, items: &mut Vec<Item>) {
    let mut floated: Vec<RawLine> = Vec::new();
    match cur {
        Some(Item::Host(b)) => take_trailing_blanks(&mut b.body, &mut floated),
        Some(Item::Match(b)) => take_trailing_blanks(&mut b.body, &mut floated),
        _ => {}
    }
    flush(cur, items);
    floated.reverse();
    for r in floated {
        items.push(Item::Blank(r));
    }
}

fn take_trailing_blanks(body: &mut Vec<BodyLine>, out: &mut Vec<RawLine>) {
    while let Some(BodyLine::Blank(_)) = body.last() {
        if let Some(BodyLine::Blank(r)) = body.pop() {
            out.push(r);
        }
    }
}

/// When a new Host/Match header is reached, pull the contiguous run of comment
/// lines directly adjacent to it (no intervening blank) out of the current
/// context — they "belong" to the upcoming block. Stops at the first blank or
/// option line. Returns them in source order.
fn steal_trailing_comments(cur: &mut Option<Item>, items: &mut Vec<Item>) -> Vec<RawLine> {
    let mut stolen: Vec<RawLine> = Vec::new();

    match cur {
        Some(Item::Host(b)) => take_trailing_from_body(&mut b.body, &mut stolen),
        Some(Item::Match(b)) => take_trailing_from_body(&mut b.body, &mut stolen),
        _ => take_trailing_from_items(items, &mut stolen),
    }

    stolen.reverse();
    stolen
}

fn take_trailing_from_body(body: &mut Vec<BodyLine>, out: &mut Vec<RawLine>) {
    while let Some(BodyLine::Comment(_)) = body.last() {
        if let Some(BodyLine::Comment(r)) = body.pop() {
            out.push(r);
        }
    }
}

fn take_trailing_from_items(items: &mut Vec<Item>, out: &mut Vec<RawLine>) {
    while let Some(Item::Comment(_)) = items.last() {
        if let Some(Item::Comment(r)) = items.pop() {
            out.push(r);
        }
    }
}
