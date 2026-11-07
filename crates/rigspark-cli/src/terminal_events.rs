use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures_util::{Stream, StreamExt, stream};
use std::{io, time::Duration};

#[derive(Default)]
struct Filter {
    string: Option<bool>,
    string_escape: bool,
    finite: Option<usize>,
    queued: Option<Event>,
}

impl Filter {
    fn accept(&mut self, event: Event) -> Option<Event> {
        if matches!(event, Event::Resize(_, _)) {
            return Some(event);
        }
        let Event::Key(key) = &event else {
            return if self.string.is_none() && self.finite.is_none() {
                Some(event)
            } else {
                None
            };
        };
        if key.kind == KeyEventKind::Release {
            return None;
        }
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if let Some(count) = self.finite {
            self.finite = if count >= 64 || matches!(key.code, KeyCode::Char('@'..='~')) {
                None
            } else {
                Some(count + 1)
            };
            return None;
        }
        if let Some(osc) = self.string {
            let terminates = key.code == KeyCode::Char('\u{9c}')
                || (key.code == KeyCode::Char('\\') && (alt || self.string_escape))
                || (osc
                    && key.code == KeyCode::Char('g')
                    && key.modifiers.contains(KeyModifiers::CONTROL));
            self.string_escape = key.code == KeyCode::Esc;
            if terminates {
                self.string = None;
                self.string_escape = false;
            }
            return None;
        }
        if matches!(key.code, KeyCode::Char('\u{9b}' | '\u{8f}')) {
            self.finite = Some(1);
            return None;
        }
        self.string = match key.code {
            KeyCode::Char(']') if alt => Some(true),
            KeyCode::Char('P' | 'X' | '^' | '_') if alt => Some(false),
            KeyCode::Char('\u{9d}') => Some(true),
            KeyCode::Char('\u{90}' | '\u{98}' | '\u{9e}' | '\u{9f}') => Some(false),
            _ => None,
        };
        if self.string.is_some() {
            None
        } else {
            Some(event)
        }
    }
}

pub(crate) fn filtered_events<Events>(events: Events) -> impl Stream<Item = io::Result<Event>>
where
    Events: Stream<Item = io::Result<Event>> + Unpin,
{
    stream::unfold(
        (events, Filter::default()),
        |(mut events, mut filter)| async move {
            loop {
                let event = match filter.queued.take() {
                    Some(event) => event,
                    None => match events.next().await? {
                        Ok(event) => event,
                        Err(error) => return Some((Err(error), (events, filter))),
                    },
                };
                if filter.string.is_none()
                    && matches!(&event, Event::Key(key) if key.code == KeyCode::Esc && key.kind != KeyEventKind::Release)
                {
                    match tokio::time::timeout(Duration::from_millis(50), events.next()).await {
                        Ok(Some(Ok(Event::Key(mut key))))
                            if matches!(key.code, KeyCode::Char(']' | 'P' | 'X' | '^' | '_')) =>
                        {
                            key.modifiers |= KeyModifiers::ALT;
                            filter.accept(Event::Key(key));
                            continue;
                        }
                        Ok(Some(Ok(next))) => filter.queued = Some(next),
                        Ok(Some(Err(error))) => return Some((Err(error), (events, filter))),
                        _ => (),
                    }
                }
                if let Some(event) = filter.accept(event) {
                    return Some((Ok(event), (events, filter)));
                }
            }
        },
    )
}

pub(crate) fn terminal_events() -> std::pin::Pin<Box<dyn Stream<Item = io::Result<Event>> + Send>> {
    Box::pin(filtered_events(EventStream::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn key(character: char, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(character), modifiers))
    }

    #[tokio::test]
    async fn terminal_strings_suppress_shortcuts_until_their_actual_terminator() {
        for (introducer, osc) in [
            (']', true),
            ('P', false),
            ('X', false),
            ('^', false),
            ('_', false),
        ] {
            for fragmented in [false, true] {
                let mut input = if fragmented {
                    vec![
                        Event::Key(KeyCode::Esc.into()),
                        key(introducer, KeyModifiers::NONE),
                    ]
                } else {
                    vec![key(introducer, KeyModifiers::ALT)]
                };
                input.extend([
                    key('q', KeyModifiers::NONE),
                    key('c', KeyModifiers::CONTROL),
                ]);
                if !osc {
                    input.extend([
                        key('g', KeyModifiers::CONTROL),
                        key('q', KeyModifiers::NONE),
                    ]);
                }
                input.extend([
                    Event::Key(KeyCode::Esc.into()),
                    key('\\', KeyModifiers::NONE),
                    key('?', KeyModifiers::NONE),
                ]);
                let output: Vec<_> = filtered_events(stream::iter(input.into_iter().map(Ok)))
                    .collect()
                    .await;
                assert_eq!(output.len(), 1, "{introducer} {fragmented}: {output:?}");
                assert_eq!(output[0].as_ref().unwrap(), &key('?', KeyModifiers::NONE));
            }
        }
    }

    #[tokio::test]
    async fn c1_strings_bel_and_paste_keep_their_boundaries() {
        for introducer in ['\u{90}', '\u{98}', '\u{9d}', '\u{9e}', '\u{9f}'] {
            let input = [
                key(introducer, KeyModifiers::NONE),
                key('q', KeyModifiers::NONE),
                key('\u{9c}', KeyModifiers::NONE),
                key('q', KeyModifiers::NONE),
            ];
            let output: Vec<_> = filtered_events(stream::iter(input.into_iter().map(Ok)))
                .collect()
                .await;
            assert_eq!(output.len(), 1);
            assert_eq!(output[0].as_ref().unwrap(), &key('q', KeyModifiers::NONE));
        }
        let paste = Event::Paste("q c\u{3}\u{1b}]title".into());
        let input = [
            key(']', KeyModifiers::ALT),
            key('q', KeyModifiers::NONE),
            key('g', KeyModifiers::CONTROL),
            paste.clone(),
            Event::Resize(80, 24),
        ];
        let output: Vec<_> = filtered_events(stream::iter(input.into_iter().map(Ok)))
            .collect()
            .await;
        assert_eq!(output.len(), 2);
        assert_eq!(output[0].as_ref().unwrap(), &paste);
        assert_eq!(output[1].as_ref().unwrap(), &Event::Resize(80, 24));
    }

    #[tokio::test]
    async fn ordinary_escape_navigation_errors_and_eof_are_preserved() {
        let input = [
            Event::Key(KeyCode::Esc.into()),
            key('[', KeyModifiers::NONE),
            key('F', KeyModifiers::SHIFT),
            key('c', KeyModifiers::CONTROL),
        ];
        let output: Vec<_> = filtered_events(stream::iter(input.clone().into_iter().map(Ok)))
            .collect()
            .await;
        assert_eq!(
            output.into_iter().map(Result::unwrap).collect::<Vec<_>>(),
            input
        );
        let output: Vec<_> = filtered_events(stream::iter([Err(io::Error::other("input failed"))]))
            .collect()
            .await;
        assert_eq!(output[0].as_ref().unwrap_err().to_string(), "input failed");
        let input = [key(']', KeyModifiers::ALT), key('q', KeyModifiers::NONE)];
        assert!(
            filtered_events(stream::iter(input.into_iter().map(Ok)))
                .collect::<Vec<_>>()
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn c1_finite_payload_is_not_a_shortcut_and_resize_is_never_suppressed() {
        for introducer in ['\u{9b}', '\u{8f}'] {
            let input = [
                key(introducer, KeyModifiers::NONE),
                key('q', KeyModifiers::NONE),
                key('?', KeyModifiers::NONE),
            ];
            let output: Vec<_> = filtered_events(stream::iter(input.into_iter().map(Ok)))
                .collect()
                .await;
            assert_eq!(output.len(), 1, "{output:?}");
            assert_eq!(output[0].as_ref().unwrap(), &key('?', KeyModifiers::NONE));
        }
        let input = [
            key(']', KeyModifiers::ALT),
            Event::Resize(1, 1),
            key('q', KeyModifiers::NONE),
            key('g', KeyModifiers::CONTROL),
            key('?', KeyModifiers::NONE),
        ];
        let output: Vec<_> = filtered_events(stream::iter(input.into_iter().map(Ok)))
            .collect()
            .await;
        assert_eq!(output.len(), 2);
        assert_eq!(output[0].as_ref().unwrap(), &Event::Resize(1, 1));
        assert_eq!(output[1].as_ref().unwrap(), &key('?', KeyModifiers::NONE));
    }

    #[tokio::test]
    async fn introducers_without_alt_or_a_leading_escape_are_ordinary_keys() {
        // Only Alt (or Esc just before) turns these into string introducers. Plain keys
        // pass through, and a key that is not Esc never consumes the key after it.
        for introducer in [']', 'P', 'X', '^', '_'] {
            let input = [
                key('a', KeyModifiers::NONE),
                key(introducer, KeyModifiers::NONE),
                key('q', KeyModifiers::NONE),
            ];
            let output: Vec<_> = filtered_events(stream::iter(input.clone().into_iter().map(Ok)))
                .collect()
                .await;
            assert_eq!(
                output.into_iter().map(Result::unwrap).collect::<Vec<_>>(),
                input,
                "{introducer}"
            );
        }
    }

    #[tokio::test]
    async fn an_error_right_after_escape_is_reported() {
        let input = [
            Ok(Event::Key(KeyCode::Esc.into())),
            Err(io::Error::other("input failed")),
        ];
        let output: Vec<_> = filtered_events(stream::iter(input)).collect().await;
        assert_eq!(output.len(), 1, "{output:?}");
        assert_eq!(output[0].as_ref().unwrap_err().to_string(), "input failed");
    }

    async fn filter(input: Vec<Event>) -> Vec<Event> {
        filtered_events(stream::iter(input.into_iter().map(Ok)))
            .map(Result::unwrap)
            .collect()
            .await
    }

    #[tokio::test]
    async fn strings_swallow_pastes_and_stray_backslashes() {
        // Only Esc-backslash ends a string; a bare backslash is payload.
        let output = filter(vec![
            key(']', KeyModifiers::ALT),
            Event::Paste("q".into()),
            key('\\', KeyModifiers::NONE),
            key('q', KeyModifiers::NONE),
            key('g', KeyModifiers::CONTROL),
            key('?', KeyModifiers::NONE),
        ])
        .await;
        assert_eq!(output, [key('?', KeyModifiers::NONE)]);
    }

    #[tokio::test]
    async fn finite_payloads_end_at_a_final_byte_or_after_64_keys() {
        let payload = |length: usize| {
            let mut input = vec![key('\u{9b}', KeyModifiers::NONE)];
            input.extend((0..length).map(|_| key('0', KeyModifiers::NONE)));
            input.extend([key('q', KeyModifiers::NONE), key('?', KeyModifiers::NONE)]);
            input
        };
        // 63 parameter keys: 'q' is still the payload's final byte.
        assert_eq!(filter(payload(63)).await, [key('?', KeyModifiers::NONE)]);
        // The 64th key ends the payload, so 'q' is an ordinary key again.
        assert_eq!(
            filter(payload(64)).await,
            [key('q', KeyModifiers::NONE), key('?', KeyModifiers::NONE)]
        );
    }
}
