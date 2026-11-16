//! Windows tools' text output the profile reads: a scheduled task's XML
//! (`schtasks /query /xml`) and the running boot entry (`bcdedit /enum
//! {current}`).

use super::TaskState;

/// The text inside the first `<tag>...</tag>` of `xml` (no attributes on
/// the opening tag, as Task Scheduler writes the elements read here).
fn xml_inner<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let len = xml[start..].find(&close)?;
    Some(xml[start..start + len].trim())
}

/// The child elements of `xml`, as their tag name and inner text, in order.
fn xml_children(xml: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(lt) = rest.find('<') {
        rest = &rest[lt + 1..];
        let end = rest
            .find(|c: char| c == '>' || c == '/' || c.is_whitespace())
            .unwrap_or(rest.len());
        let name = &rest[..end];
        let Some(gt) = rest.find('>') else { break };
        if rest[..gt].ends_with('/') {
            // Self-closing: the next `<` is the next element.
            out.push((name, ""));
            continue;
        }
        let body = &rest[gt + 1..];
        let close = format!("</{name}>");
        let Some(c) = body.find(&close) else { break };
        out.push((name, body[..c].trim()));
        rest = &body[c + close.len()..];
    }
    out
}

/// The `HH:MM` of a trigger's `StartBoundary` (`2026-11-05T04:00:00+01:00`),
/// as written: the task's own local time.
fn boundary_time(trigger: &str) -> String {
    xml_inner(trigger, "StartBoundary")
        .and_then(|b| b.split_once('T'))
        .and_then(|(_, t)| t.get(..5))
        .unwrap_or("?")
        .to_string()
}

/// A scheduled task's state from `schtasks /query /xml`: on unless its
/// `Settings` say `<Enabled>false</Enabled>`; each trigger as a short line:
/// `daily HH:MM`, `every Nd HH:MM`, `weekly HH:MM`, `monthly HH:MM`,
/// `time HH:MM`, `boot`, `logon`, `idle`, `event`, else `other:<Tag>`; with
/// ` every <interval>` when it repeats (e.g. `every PT2H`) and ` (off)` when
/// the trigger is disabled.
pub fn parse_task_xml(xml: &str) -> TaskState {
    let enabled = xml_inner(xml, "Settings")
        .and_then(|s| xml_children(s).into_iter().find(|(n, _)| *n == "Enabled"))
        .is_none_or(|(_, v)| !v.eq_ignore_ascii_case("false"));
    let triggers = xml_inner(xml, "Triggers")
        .map(|t| {
            xml_children(t)
                .into_iter()
                .map(|(name, body)| {
                    let at = boundary_time(body);
                    let mut line = match name {
                        "CalendarTrigger" => {
                            if let Some(d) = xml_inner(body, "ScheduleByDay") {
                                match xml_inner(d, "DaysInterval").unwrap_or("1") {
                                    "1" => format!("daily {at}"),
                                    n => format!("every {n}d {at}"),
                                }
                            } else if body.contains("<ScheduleByWeek>") {
                                format!("weekly {at}")
                            } else if body.contains("<ScheduleByMonth") {
                                format!("monthly {at}")
                            } else {
                                format!("other:{name}")
                            }
                        }
                        "TimeTrigger" => format!("time {at}"),
                        "BootTrigger" => "boot".into(),
                        "LogonTrigger" => "logon".into(),
                        "IdleTrigger" => "idle".into(),
                        "EventTrigger" => "event".into(),
                        other => format!("other:{other}"),
                    };
                    if let Some(i) =
                        xml_inner(body, "Repetition").and_then(|r| xml_inner(r, "Interval"))
                    {
                        line.push_str(&format!(" every {i}"));
                    }
                    if xml_children(body)
                        .iter()
                        .any(|(n, v)| *n == "Enabled" && v.eq_ignore_ascii_case("false"))
                    {
                        line.push_str(" (off)");
                    }
                    line
                })
                .collect()
        })
        .unwrap_or_default();
    TaskState { enabled, triggers }
}

/// The settings of the entry `bcdedit /enum {current}` printed: each line
/// after the dashed line under the entry's title, as its first word and the
/// rest. A value bcdedit continues on indented lines (a list) keeps only its
/// first line; element names are never localized, the title is skipped.
pub fn parse_bcdedit(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut body = false;
    for line in text.lines() {
        let t = line.trim_end();
        if !body {
            body = !t.is_empty() && t.chars().all(|c| c == '-');
            continue;
        }
        if t.is_empty() || t.starts_with(char::is_whitespace) {
            continue;
        }
        let mut parts = t.splitn(2, char::is_whitespace);
        let (Some(name), Some(value)) = (parts.next(), parts.next()) else {
            continue;
        };
        out.push((name.to_string(), value.trim().to_string()));
    }
    out
}
