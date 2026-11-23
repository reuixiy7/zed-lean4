//! Line-checkpointed lexical context for abbreviation input. This is deliberately
//! independent of Lean elaboration, since it must also work in unfinished code.

#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub(super) struct Context(Vec<Mode>);

#[derive(Clone, PartialEq, Eq, Debug)]
enum Mode {
    String { escaped: bool, interpolated: bool },
    RawString(usize),
    Character { escaped: bool },
    BlockComment(usize),
    LineComment,
    Interpolation(usize),
    QuotedIdentifier,
}

impl Context {
    pub fn allows_abbreviation(&self) -> bool {
        !matches!(
            self.0.last(),
            Some(Mode::String { .. } | Mode::RawString(_) | Mode::Character { .. })
        )
    }

    pub fn scan(&mut self, text: &str) {
        let mut i = 0;
        while i < text.len() {
            let tail = &text[i..];
            let c = tail.chars().next().unwrap();
            let mut consumed = c.len_utf8();
            match self.0.last_mut() {
                Some(Mode::LineComment) => {
                    if c == '\n' {
                        self.0.pop();
                    }
                }
                Some(Mode::BlockComment(depth)) => {
                    if tail.starts_with("/-") {
                        *depth += 1;
                        consumed = 2;
                    } else if tail.starts_with("-/") {
                        *depth -= 1;
                        if *depth == 0 {
                            self.0.pop();
                        }
                        consumed = 2;
                    }
                }
                Some(Mode::RawString(hashes)) => {
                    if c == '"'
                        && tail
                            .as_bytes()
                            .get(1..1 + *hashes)
                            .is_some_and(|s| s.iter().all(|&b| b == b'#'))
                    {
                        consumed += *hashes;
                        self.0.pop();
                    }
                }
                Some(Mode::String {
                    escaped,
                    interpolated,
                }) => {
                    if *escaped {
                        *escaped = false;
                    } else if c == '\\' {
                        *escaped = true;
                    } else if c == '"' {
                        self.0.pop();
                    } else if c == '{' && *interpolated {
                        self.0.push(Mode::Interpolation(0));
                    }
                }
                Some(Mode::Character { escaped }) => {
                    if c == '\n' {
                        self.0.pop();
                    } else if *escaped {
                        *escaped = false;
                    } else if c == '\\' {
                        *escaped = true;
                    } else if c == '\'' {
                        self.0.pop();
                    }
                }
                Some(Mode::QuotedIdentifier) => {
                    if c == '»' {
                        self.0.pop();
                    }
                }
                _ => {
                    if tail.starts_with("--") {
                        self.0.push(Mode::LineComment);
                        consumed = 2;
                    } else if tail.starts_with("/-") {
                        self.0.push(Mode::BlockComment(1));
                        consumed = 2;
                    } else if c == 'r'
                        && let Some(hashes) = raw_start(tail)
                    {
                        self.0.push(Mode::RawString(hashes));
                        consumed = hashes + 2;
                    } else if c == '"' {
                        // Lean's s!, m!, f!, etc. use the same interpolation syntax.
                        let interpolated = text[..i].ends_with('!');
                        self.0.push(Mode::String {
                            escaped: false,
                            interpolated,
                        });
                    } else if c == '\''
                        && !text[..i]
                            .chars()
                            .next_back()
                            .is_some_and(|c| c.is_alphanumeric() || "_'»".contains(c))
                    {
                        self.0.push(Mode::Character { escaped: false });
                    } else if c == '«' {
                        self.0.push(Mode::QuotedIdentifier);
                    } else if let Some(Mode::Interpolation(depth)) = self.0.last_mut() {
                        if c == '{' {
                            *depth += 1;
                        } else if c == '}' {
                            if *depth == 0 {
                                self.0.pop();
                            } else {
                                *depth -= 1;
                            }
                        }
                    }
                }
            }
            i += consumed;
        }
    }
}

fn raw_start(text: &str) -> Option<usize> {
    let hashes = text.as_bytes()[1..]
        .iter()
        .take_while(|&&b| b == b'#')
        .count();
    (text.as_bytes().get(hashes + 1) == Some(&b'"')).then_some(hashes)
}
