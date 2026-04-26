//! `CliRenderer` — projects a [`CliOutput`] onto stdout.
//!
//! Two adapters cover the user-facing modes:
//! * `HumanRenderer` — table + key/value layout for terminals. Default.
//! * `JsonRenderer`  — one JSON document per command. Used when the
//!   operator passes `--json` (or pipes into `jq`).
//!
//! Commands stay agnostic of the rendering: they return a `CliOutput`
//! and the renderer decides how to print it. New mode (e.g. NDJSON
//! streaming, YAML) lands as one more renderer adapter, no command
//! changes.

use std::io::{self, Write};

use crate::cli::command::CliOutput;

/// Strategy that turns `CliOutput` into bytes on a writer (typically
/// `stdout`). Implementations are passed into [`CliContext`] before
/// dispatch.
pub trait CliRenderer: Send + Sync {
    /// Render the output. Errors here propagate up so dispatch can map
    /// to a non-zero exit code if `stdout` is broken.
    fn render(&self, out: &CliOutput) -> io::Result<()>;
}

/// Default renderer: aligned tables, key/value blocks, and pre-formatted
/// `Lines` flushed verbatim. JSON output is pretty-printed when this
/// renderer is asked to handle it (rare — usually JSON commands install
/// `JsonRenderer` instead).
pub struct HumanRenderer;

impl CliRenderer for HumanRenderer {
    fn render(&self, out: &CliOutput) -> io::Result<()> {
        let stdout = io::stdout();
        let mut w = stdout.lock();
        match out {
            CliOutput::Silent => Ok(()),
            CliOutput::KeyValue(pairs) => {
                let pad = pairs.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
                for (k, v) in pairs {
                    writeln!(w, "{k:<width$}  {v}", width = pad)?;
                }
                Ok(())
            }
            CliOutput::Table { headers, rows } => {
                let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
                for row in rows {
                    for (i, cell) in row.iter().enumerate() {
                        if i >= widths.len() {
                            widths.push(cell.len());
                        } else if cell.len() > widths[i] {
                            widths[i] = cell.len();
                        }
                    }
                }
                let render_row = |row: &[String], w: &mut dyn Write| -> io::Result<()> {
                    for (i, cell) in row.iter().enumerate() {
                        let width = widths.get(i).copied().unwrap_or(cell.len());
                        if i + 1 == row.len() {
                            write!(w, "{cell}")?;
                        } else {
                            write!(w, "{cell:<width$}  ")?;
                        }
                    }
                    writeln!(w)
                };
                render_row(headers, &mut w as &mut dyn Write)?;
                let total: usize = widths.iter().sum::<usize>() + widths.len().saturating_sub(1) * 2;
                writeln!(w, "{}", "─".repeat(total.max(1)))?;
                for row in rows {
                    render_row(row, &mut w as &mut dyn Write)?;
                }
                Ok(())
            }
            CliOutput::Json(v) => {
                writeln!(w, "{}", serde_json::to_string_pretty(v).unwrap_or_default())
            }
            CliOutput::Lines(lines) => {
                for line in lines {
                    writeln!(w, "{line}")?;
                }
                Ok(())
            }
        }
    }
}

/// JSON-only renderer. Every output kind collapses to a single JSON
/// document with a `kind` discriminator + `data` payload — operators
/// piping into `jq` get a stable shape regardless of which command
/// they ran.
pub struct JsonRenderer;

impl CliRenderer for JsonRenderer {
    fn render(&self, out: &CliOutput) -> io::Result<()> {
        let stdout = io::stdout();
        let mut w = stdout.lock();
        let payload = match out {
            CliOutput::Silent => serde_json::json!({"kind": "silent"}),
            CliOutput::KeyValue(pairs) => {
                let map: serde_json::Map<String, serde_json::Value> = pairs
                    .iter()
                    .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                    .collect();
                serde_json::json!({"kind": "key_value", "data": map})
            }
            CliOutput::Table { headers, rows } => {
                serde_json::json!({"kind": "table", "headers": headers, "rows": rows})
            }
            CliOutput::Json(v) => v.clone(),
            CliOutput::Lines(lines) => {
                serde_json::json!({"kind": "lines", "data": lines})
            }
        };
        writeln!(
            w,
            "{}",
            serde_json::to_string(&payload).unwrap_or_default()
        )
    }
}
