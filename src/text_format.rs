// Plain text format parser/formatter — the .fmscript format.
// This is the human-readable representation shown in VSCode.
// Format: one step per line (or multiline for calculations), 2-space indent.

use crate::steps::{self, StepShape};
use crate::xmss::{FmScript, ScriptStep};

/// Format a script as plain text for display/editing.
pub fn format_script(script: &FmScript) -> String {
    let mut lines = Vec::new();
    for step in &script.steps {
        lines.push(format_step(step));
    }
    lines.join("\n")
}

/// Format a single step as a text line (possibly multiline for calculations).
pub fn format_step(step: &ScriptStep) -> String {
    let indent = "  ".repeat(step.indent_level as usize);

    // Comments are special: "# text".
    // A comment with no <Text> child in FM is a truly blank line the user added
    // by pressing Enter in the Script Workspace — render it as an empty line so
    // it survives round-trip without becoming `# `.
    // FM stores Enter-inside-a-comment as `&#13;`. Quick-xml decodes that to `\r`
    // and we normalize to `\n`. Re-encode as the `&#13;` sigil so each comment
    // stays on a single .fmscript line.
    if step.name == steps::COMMENT_NAME {
        return match step.text.as_deref() {
            None => String::new(),
            Some(t) => format!("{}# {}", indent, t.replace('\n', "&#13;")),
        };
    }

    let prefix = if step.enable { "" } else { "// " };
    let mut line = format!("{}{}{}", indent, prefix, step.name);

    let shape = steps::shape_for_en(&step.name);
    match shape {
        Some(StepShape::ValueCalcName) => {
            let mut parts = Vec::new();
            if let Some(name) = &step.var_name {
                parts.push(name.clone());
            }
            if let Some(calc) = &step.calculation {
                parts.push(format!("= {}", calc.trim()));
            }
            if !parts.is_empty() {
                line.push_str(&format!(" [{}]", parts.join(" ")));
            }
        }
        Some(StepShape::SetState) => {
            if let Some(state) = &step.set_state {
                line.push_str(&format!(" [{}]", state));
            }
        }
        Some(StepShape::Calculation) | Some(StepShape::CalculationWithRestore) => {
            if let Some(calc) = &step.calculation {
                let trimmed = calc.trim();
                if !trimmed.is_empty() {
                    // Check if calculation is multiline
                    if trimmed.contains('\n') {
                        line.push_str(" [");
                        // For multiline, the closing ] goes on the last line
                        let calc_lines: Vec<&str> = trimmed.lines().collect();
                        for (i, cl) in calc_lines.iter().enumerate() {
                            if i > 0 {
                                line.push('\n');
                                line.push_str(&indent);
                                line.push_str("  ");
                            }
                            line.push_str(cl);
                        }
                        line.push(']');
                    } else {
                        line.push_str(&format!(" [{}]", trimmed));
                    }
                }
            }
        }
        Some(StepShape::Dialog) => {
            let mut parts = Vec::new();
            if let Some(title) = &step.dialog_title {
                let t = title.trim();
                if !t.is_empty() {
                    parts.push(format!("Title: {}", t));
                }
            }
            if let Some(msg) = &step.dialog_message {
                let m = msg.trim();
                if !m.is_empty() {
                    parts.push(format!("Message: {}", m));
                }
            }
            if !step.dialog_buttons.is_empty() {
                let btns: Vec<String> = step.dialog_buttons.iter()
                    .filter(|b| !b.trim().is_empty())
                    .map(|b| b.trim().to_string())
                    .collect();
                if !btns.is_empty() {
                    parts.push(format!("Buttons: {}", btns.join(", ")));
                }
            }
            if !parts.is_empty() {
                line.push_str(&format!(" [{}]", parts.join("; ")));
            }
        }
        Some(StepShape::WebViewerJs) => {
            let mut parts = Vec::new();
            if let Some(obj) = &step.object_name {
                parts.push(format!("Object: {}", obj.trim()));
            }
            if let Some(func) = &step.function_name {
                parts.push(format!("Function: {}", func.trim()));
            }
            for (i, p) in step.parameters.iter().enumerate() {
                parts.push(format!("Param[{}]: {}", i, p.trim()));
            }
            if !parts.is_empty() {
                line.push_str(&format!(" [{}]", parts.join("; ")));
            }
        }
        Some(StepShape::FieldByName) => {
            let mut parts = Vec::new();
            if let Some(result) = &step.field_result {
                parts.push(format!("Result: {}", result.trim()));
            }
            if let Some(target) = &step.field_target {
                parts.push(format!("Target: {}", target.trim()));
            }
            if !parts.is_empty() {
                line.push_str(&format!(" [{}]", parts.join("; ")));
            }
        }
        Some(StepShape::SelectWindow) => {
            // The name is a FM calc expression (literal "X" or any expr like $var),
            // so show it verbatim. Mode keyword (Current/First/...) shown when no name.
            if let Some(name) = &step.var_name {
                line.push_str(&format!(" [{}]", name));
            } else if let Some(mode) = &step.window_mode {
                line.push_str(&format!(" [{}]", mode));
            }
        }
        Some(StepShape::AdjustWindow) => {
            if let Some(state) = &step.window_state {
                line.push_str(&format!(" [{}]", state));
            }
        }
        Some(StepShape::DataApi) => {
            // Execute FileMaker Data API: [$target; query_calc]
            let calc = step.calculation.as_deref().map(|c| c.trim()).unwrap_or("");
            match (&step.field_target, calc.is_empty()) {
                (Some(t), false) => line.push_str(&format!(" [{}; {}]", t, calc)),
                (Some(t), true)  => line.push_str(&format!(" [{};]", t)),
                (None, false)    => line.push_str(&format!(" [{}]", calc)),
                (None, true)     => {}
            }
        }
        Some(StepShape::GoToRecord) => {
            // Format: [Location; Exit; NoInteract] — only includes flags that are True.
            // For byCalculation: [Calc: <expr>; ...flags...].
            let mut parts: Vec<String> = Vec::new();
            if let Some(loc) = &step.goto_location {
                if loc == "byCalculation" {
                    let calc = step.calculation.as_deref().map(|c| c.trim()).unwrap_or("");
                    parts.push(format!("Calc: {}", calc));
                } else {
                    parts.push(loc.clone());
                }
            }
            if step.goto_exit_after_last.as_deref() == Some("True") {
                parts.push("Exit".to_string());
            }
            if step.goto_no_interact.as_deref() == Some("True") {
                parts.push("NoInteract".to_string());
            }
            if !parts.is_empty() {
                line.push_str(&format!(" [{}]", parts.join("; ")));
            }
        }
        Some(StepShape::PerformScript) => {
            // Perform Script: shows ["Name" #id; param]. The #id suffix is what
            // lets FM resolve the link on paste — name alone is just display.
            let calc = step.calculation.as_deref().map(|c| c.trim()).unwrap_or("");
            let name_part = match (&step.script_target_name, &step.script_target_id) {
                (Some(n), Some(id)) => format!("\"{}\" #{}", n, id),
                (Some(n), None)     => format!("\"{}\"", n),
                (None, _)           => String::new(),
            };
            match (name_part.is_empty(), calc.is_empty()) {
                (false, false) => line.push_str(&format!(" [{}; {}]", name_part, calc)),
                (false, true)  => line.push_str(&format!(" [{}]", name_part)),
                (true, false)  => line.push_str(&format!(" [{}]", calc)),
                (true, true)   => {}
            }
        }
        Some(StepShape::FieldAndCalc) => {
            // Set Field: "[Table::Name; calc]". No numeric ID — FM resolves the field
            // by table+name on paste. This lets AI/humans author scripts from scratch
            // without having to discover FM's internal IDs.
            let target_display: Option<String> = match (&step.field_table, &step.field_target) {
                (Some(t), Some(n)) => Some(format!("{}::{}", t, n)),
                (None, Some(n)) => Some(n.clone()),
                _ => None,
            };
            let calc = step.calculation.as_deref().map(|c| c.trim()).unwrap_or("");
            match (target_display, calc.is_empty()) {
                (Some(tgt), false) => line.push_str(&format!(" [{}; {}]", tgt, calc)),
                (Some(tgt), true)  => line.push_str(&format!(" [{};]", tgt)),
                (None, false)      => line.push_str(&format!(" [{}]", calc)),
                (None, true)       => {}
            }
        }
        Some(StepShape::ReplaceFieldContents) => {
            // Like Set Field — `[Table::Field; calc]` — plus a trailing `Dialog: Off`
            // when the dialog is suppressed. Parts joined with `; `.
            let target_display: Option<String> = match (&step.field_table, &step.field_target) {
                (Some(t), Some(n)) => Some(format!("{}::{}", t, n)),
                (None, Some(n)) => Some(n.clone()),
                _ => None,
            };
            let calc = step.calculation.as_deref().map(|c| c.trim()).unwrap_or("");
            let mut parts: Vec<String> = Vec::new();
            if let Some(tgt) = target_display { parts.push(tgt); }
            if !calc.is_empty() { parts.push(calc.to_string()); }
            if step.goto_no_interact.as_deref() == Some("True") {
                parts.push("Dialog: Off".to_string());
            }
            if !parts.is_empty() {
                line.push_str(&format!(" [{}]", parts.join("; ")));
            }
        }
        Some(StepShape::GoToObject) => {
            // `Go to Object [name]` or `[name; Rep: N]` when repetition ≠ 1.
            let obj = step.object_name.as_deref().map(|s| s.trim()).unwrap_or("");
            let rep = step.repetition.as_deref().map(|s| s.trim()).unwrap_or("1");
            if obj.is_empty() && rep == "1" {
                // nothing useful — emit no brackets
            } else if rep == "1" || rep.is_empty() {
                line.push_str(&format!(" [{}]", obj));
            } else {
                line.push_str(&format!(" [{}; Rep: {}]", obj, rep));
            }
        }
        Some(StepShape::GoToLayoutNamed) => {
            // `Go to Layout ["Name" #id]` (round-trip) or `["Name"]` (from-scratch);
            // `[original]` for OriginalLayout.
            let dest = step.layout_destination.as_deref().unwrap_or("SelectedLayout");
            if dest == "OriginalLayout" {
                line.push_str(" [original]");
            } else if let Some(name) = &step.layout_name {
                match &step.layout_id {
                    Some(id) => line.push_str(&format!(" [\"{}\" #{}]", name, id)),
                    None => line.push_str(&format!(" [\"{}\"]", name)),
                }
            }
        }
        Some(StepShape::NewWindow) => {
            // `New Window [Style: Document; Layout: "X"; Height: 1; Width: 1; Top: -1000; Left: -1000]`
            // All fields optional; emit only what is set.
            let mut parts: Vec<String> = Vec::new();
            if let Some(s) = &step.window_style_name { parts.push(format!("Style: {}", s.trim())); }
            if let Some(l) = &step.layout_name       { parts.push(format!("Layout: \"{}\"", l)); }
            if let Some(h) = &step.window_height     { let t = h.trim(); if !t.is_empty() { parts.push(format!("Height: {}", t)); } }
            if let Some(w) = &step.window_width      { let t = w.trim(); if !t.is_empty() { parts.push(format!("Width: {}", t)); } }
            if let Some(t) = &step.window_top        { let v = t.trim(); if !v.is_empty() { parts.push(format!("Top: {}", v)); } }
            if let Some(l) = &step.window_left       { let v = l.trim(); if !v.is_empty() { parts.push(format!("Left: {}", v)); } }
            if !parts.is_empty() {
                line.push_str(&format!(" [{}]", parts.join("; ")));
            }
        }
        Some(StepShape::InsertFromUrl) => {
            // [Target: ...; URL: ...; cURL: "..."; Dialog: Off; VerifySSL; SelectAll; DontEncode]
            // Flags emit only when non-default (FM default for all 4 flag fields is False).
            // Dialog: Off ↔ NoInteract=True. Dialog defaults On so emit nothing when NoInteract=False.
            let mut parts: Vec<String> = Vec::new();
            if let Some(t) = &step.field_target {
                let qualified = match &step.field_table {
                    Some(tb) if !t.starts_with('$') => format!("{}::{}", tb, t),
                    _ => t.clone(),
                };
                parts.push(format!("Target: {}", qualified));
            }
            if let Some(url) = &step.calculation {
                let u = url.trim();
                if !u.is_empty() { parts.push(format!("URL: {}", u)); }
            }
            if let Some(curl) = &step.curl_options {
                let c = curl.trim();
                if !c.is_empty() { parts.push(format!("cURL: {}", c)); }
            }
            if step.goto_no_interact.as_deref() == Some("True") {
                parts.push("Dialog: Off".to_string());
            }
            if step.verify_ssl.as_deref() == Some("True")  { parts.push("VerifySSL".to_string()); }
            if step.select_all_state.as_deref() == Some("True") { parts.push("SelectAll".to_string()); }
            if step.dont_encode_url.as_deref() == Some("True")  { parts.push("DontEncode".to_string()); }
            if !parts.is_empty() {
                line.push_str(&format!(" [{}]", parts.join("; ")));
            }
        }
        Some(StepShape::PerformFind) => {
            // Multi-line by default for readability. Each request becomes one section:
            //   Find: T::F1 => v1; T::F2 => v2
            //   Omit: T::F3 => v3
            // Sections are on their own line; criteria within a section are `;`-separated.
            if step.find_requests.is_empty() {
                // nothing to render
            } else {
                line.push_str(" [");
                let line_indent = "  ".repeat(step.indent_level as usize);
                let cont_indent = format!("{}  ", line_indent);
                for req in &step.find_requests {
                    let header = if req.operation == "Omit" { "Omit" } else { "Find" };
                    let crits: Vec<String> = req.criteria.iter()
                        .map(|c| {
                            let target = if c.table.is_empty() { c.field.clone() } else { format!("{}::{}", c.table, c.field) };
                            format!("{} => {}", target, c.text)
                        })
                        .collect();
                    line.push('\n');
                    line.push_str(&cont_indent);
                    line.push_str(&format!("{}: {}", header, crits.join("; ")));
                }
                line.push('\n');
                line.push_str(&line_indent);
                line.push(']');
            }
        }
        Some(StepShape::Comment) | Some(StepShape::Plain) | Some(StepShape::Opaque) => {
            // Fallback: show any calc or text we have.
            // Opaque steps carry their full inner FM XML verbatim in `calculation`.
            if let Some(calc) = &step.calculation {
                let trimmed = calc.trim();
                if !trimmed.is_empty() {
                    line.push_str(&format!(" [{}]", trimmed));
                }
            } else if let Some(text) = &step.text {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    line.push_str(&format!(" [{}]", trimmed));
                }
            }
        }
        None => {
            // Unrecognized step → opaque by default. It isn't in steps.toml, so we
            // carry its FM step id inline (`Name #id`) for the round-trip, followed
            // by its raw inner XML verbatim in brackets. parse_text_to_script reads
            // the inline id back; the linter treats it as valid (not an error).
            line.push_str(&format!(" #{}", step.id));
            if let Some(calc) = &step.calculation {
                let trimmed = calc.trim();
                if !trimmed.is_empty() {
                    line.push_str(&format!(" [{}]", trimmed));
                }
            }
        }
    }

    // Editor-friendly disabled steps: when the rendered step spans multiple lines,
    // the per-line `// ` prefix only marks the first line — VSCode and similar editors
    // then treat continuation lines as live code, and a stray `"` in the first line
    // leaks as an unclosed string through the rest of the file. Re-wrap as
    // `/* ... */` so the block comment encompasses every line. Single-line disabled
    // steps stay as `// step [...]` (no editor confusion possible).
    if !step.enable && line.contains('\n') {
        if let Some(rest) = line.strip_prefix(&format!("{}// ", indent)) {
            line = format!("{}/* {}\n{}*/", indent, rest, indent);
        }
    }

    line
}

/// Rewrite `/* ... */` block comments into per-line `// ` prefixes so the
/// rest of the parser can stay single-line-aware. Block comments are the
/// editor-friendly form for disabling multi-line steps (a `// ` on the first
/// line lets stray `"` in the calc leak as unclosed strings through VSCode).
fn preprocess_block_comments(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_block = false;
    for raw in text.lines() {
        let leading_ws_len = raw.len() - raw.trim_start().len();
        let leading_ws = &raw[..leading_ws_len];
        let body = raw.trim_start();

        if !in_block {
            if let Some(after_open) = body.strip_prefix("/* ").or_else(|| body.strip_prefix("/*")) {
                // A `/* ... */` that disables a whole step (the only shape this tool
                // ever generates, see `format_step` above) always has the step name
                // right after `/* `, e.g. `/* Perform Script [...] */`. A calculation
                // can also contain a *literal*, hand-authored FileMaker in-formula
                // comment nested inside an already-open, still-active step (e.g. to
                // comment out a couple of JSONSetElement rows while keeping the call
                // itself live) — that shape starts with `[` right after `/* `, with no
                // step name. Treat that case as plain text: leave it untouched so the
                // enclosing step's own bracket-matching finds its real closing `]`,
                // instead of misreading it as a step-disable marker and swallowing
                // whatever follows the closing `*/` on its last line.
                if after_open.trim_start().starts_with('[') {
                    out.push(raw.to_string());
                    continue;
                }
                in_block = true;
                // Check for `*/` on the same line (single-line block comment).
                if let Some(close_idx) = after_open.rfind("*/") {
                    let inner = after_open[..close_idx].trim_end();
                    out.push(format!("{}// {}", leading_ws, inner));
                    in_block = false;
                } else {
                    out.push(format!("{}// {}", leading_ws, after_open));
                }
            } else {
                out.push(raw.to_string());
            }
        } else if let Some(close_idx) = raw.rfind("*/") {
            // Closing line — keep any content before `*/`.
            let kept = raw[..close_idx].trim_end();
            if !kept.is_empty() {
                out.push(kept.to_string());
            }
            in_block = false;
        } else {
            out.push(raw.to_string());
        }
    }
    out.join("\n")
}

/// A parse failure with the 1-based source line it occurred on (`0` = unknown).
/// Carries structured position info so the VS Code extension can place a
/// diagnostic on the right line, not just show a flat message.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line > 0 {
            write!(f, "Line {}: {}", self.line, self.message)
        } else {
            write!(f, "{}", self.message)
        }
    }
}

/// String errors (e.g. from `resolve_id`) become line-less ParseErrors; call
/// sites that know the line override it explicitly via `map_err`.
impl From<String> for ParseError {
    fn from(message: String) -> Self {
        ParseError { line: 0, message }
    }
}

/// Parse plain text into a structured script.
/// Handles multiline bracket content by collecting lines until `]` is found.
pub fn parse_text_to_script(text: &str) -> Result<FmScript, ParseError> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let preprocessed = preprocess_block_comments(text);
    let text = preprocessed.as_str();
    let mut steps = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;

    let resolve_id = |name: &str| -> Result<u32, String> {
        steps::id_for_en(name).ok_or_else(|| unknown_step_message(name))
    };

    while i < lines.len() {
        let line = lines[i];
        // Blank line → emit an empty comment step. FM represents the user pressing
        // Enter in the Script Workspace as `<Step name="# (comment)"></Step>` with
        // no <Text> child, and we want that to round-trip.
        if line.trim().is_empty() {
            steps.push(ScriptStep {
                name: steps::COMMENT_NAME.to_string(),
                enable: true,
                id: resolve_id(steps::COMMENT_NAME)?,
                text: None,
                calculation: None, var_name: None, repetition: None,
                object_name: None, function_name: None, parameters: Vec::new(),
                restore_state: None, set_state: None,
                dialog_title: None, dialog_message: None, dialog_buttons: Vec::new(),
                field_result: None, field_target: None, field_table: None, field_numeric_id: None,
                script_target_name: None, script_target_id: None, current_script_mode: None,
                goto_location: None, goto_exit_after_last: None, goto_no_interact: None,
                window_mode: None, window_limit_current_file: None, window_state: None,
                layout_name: None, layout_id: None, layout_destination: None,
                window_height: None, window_width: None, window_top: None, window_left: None, window_style_name: None,
                find_requests: Vec::new(),
                curl_options: None, dont_encode_url: None, verify_ssl: None, select_all_state: None,
                indent_level: 0,
            });
            i += 1;
            continue;
        }

        let leading_spaces = line.len() - line.trim_start().len();
        let indent = (leading_spaces / 2) as u32;

        let enabled = !line.trim().starts_with("// ");
        let content = if line.trim().starts_with("// ") { &line.trim()[3..] } else { line.trim() };

        // Comment lines
        if content.starts_with('#') {
            // Reverse of the format_step `\n` → `&#13;` sigil.
            let comment_text = content.strip_prefix("# ").unwrap_or("").replace("&#13;", "\n");
            steps.push(ScriptStep {
                name: steps::COMMENT_NAME.to_string(),
                enable: true,
                id: resolve_id(steps::COMMENT_NAME)?,
                text: if comment_text.is_empty() { None } else { Some(comment_text) },
                calculation: None, var_name: None, repetition: None,
                object_name: None, function_name: None, parameters: Vec::new(),
                restore_state: None, set_state: None,
                dialog_title: None, dialog_message: None, dialog_buttons: Vec::new(),
                field_result: None, field_target: None, field_table: None, field_numeric_id: None,
                script_target_name: None, script_target_id: None, current_script_mode: None,
                goto_location: None, goto_exit_after_last: None, goto_no_interact: None,
                window_mode: None, window_limit_current_file: None, window_state: None,
                layout_name: None, layout_id: None, layout_destination: None,
                window_height: None, window_width: None, window_top: None, window_left: None, window_style_name: None,
                find_requests: Vec::new(),
                curl_options: None, dont_encode_url: None, verify_ssl: None, select_all_state: None,
                indent_level: indent,
            });
            i += 1;
            continue;
        }

        // Check for bracket content (single or multiline).
        // Depth-aware scan: brackets inside string literals (`"..."`) and balanced
        // `[...]` pairs (JSONSetElement rows, List() literals, etc.) don't terminate
        // the step's bracket. We close only when an unmatched `]` is found.
        if let Some(idx) = content.find(" [") {
            let step_line = i; // 0-based line where this step (and its `[`) begins
            // Accept Spanish step names too: translate to the English canonical
            // name before any shape/id lookup (decode does the same for XML).
            // English names pass through unchanged.
            let (name_token, inline_id) = split_inline_step_id(&content[..idx]);
            let step_name = steps::translate_to_en(name_token);
            let first_chunk = &content[idx + 2..];

            // The formatter adds `indent + 2` leading spaces to continuation lines
            // for CalculationWithRestore so the multi-line `If [...]` reads cleanly.
            // Other shapes embed user-authored calcs verbatim, so dedenting them
            // would destroy intentional indentation (e.g. Let blocks in Set Variable).
            let dedent_continuations = matches!(
                steps::shape_for_en(&step_name),
                Some(StepShape::CalculationWithRestore)
            );

            let mut bracket_content = String::new();
            let mut depth: i32 = 0;
            let mut in_string = false;
            let mut terminated = false;
            let mut current_text: &str = first_chunk;

            loop {
                for ch in current_text.chars() {
                    match ch {
                        '"' => { in_string = !in_string; bracket_content.push(ch); }
                        '[' if !in_string => { depth += 1; bracket_content.push(ch); }
                        ']' if !in_string => {
                            if depth == 0 { terminated = true; break; }
                            depth -= 1;
                            bracket_content.push(ch);
                        }
                        _ => bracket_content.push(ch),
                    }
                }
                if terminated { break; }
                i += 1;
                if i >= lines.len() {
                    return Err(ParseError {
                        line: step_line + 1,
                        message: format!("Unclosed `[` in step '{}'", step_name),
                    });
                }
                bracket_content.push('\n');
                let raw_line = lines[i];
                current_text = if dedent_continuations {
                    // Eat up to `leading_spaces + 2` leading spaces (matches the
                    // formatter's added indent). Tabs are never eaten.
                    let cont_dedent = leading_spaces + 2;
                    let strip = raw_line.chars().take(cont_dedent).take_while(|c| *c == ' ').count();
                    &raw_line[strip..]
                } else {
                    raw_line
                };
            }
            i += 1;

            // Known steps resolve via steps.toml; an unrecognized step falls back
            // to its inline `#id` (opaque-by-default) instead of erroring.
            let id = steps::id_for_en(&step_name)
                .or(inline_id)
                .ok_or_else(|| unknown_step_message(&step_name))
                .map_err(|m| ParseError { line: step_line + 1, message: m })?;
            let step = build_step_from_name(&step_name, Some(&bracket_content), enabled, id, indent);
            steps.push(step);
        } else {
            // No bracket content. Translate Spanish → English canonical first.
            let (name_token, inline_id) = split_inline_step_id(content);
            let step_name = steps::translate_to_en(name_token);
            let id = steps::id_for_en(&step_name)
                .or(inline_id)
                .ok_or_else(|| unknown_step_message(&step_name))
                .map_err(|m| ParseError { line: i + 1, message: m })?;
            let step = build_step_from_name(&step_name, None, enabled, id, indent);
            steps.push(step);
            i += 1;
        }
    }

    Ok(FmScript { steps })
}

/// The actionable message for a step name that isn't in steps.toml (or lacks an
/// id). Shared by the parser and the linter so both speak the same language.
/// When the name looks like a casing/spacing slip of a real step, it points at
/// the likely intended one ("did you mean 'If'?") so the editor can offer a fix.
fn unknown_step_message(name: &str) -> String {
    if let Some(suggestion) = steps::closest_en(name) {
        return format!("Unknown step '{}' — did you mean '{}'?", name, suggestion);
    }
    format!(
        "Step '{}' has no FileMaker ID in steps.toml. \
         Copy this step in FileMaker and run `fm-bridge dump-ids` to discover its id, \
         then add `id = N` to its entry in steps.toml.",
        name
    )
}

/// Split a trailing ` #<digits>` step-id off a step-name token. Unrecognized
/// (opaque) steps carry their FileMaker step id inline because steps.toml has no
/// entry to resolve it from. Returns (name_without_id, Some(id)) when the suffix
/// is present and numeric, else (original, None). Known steps never use this —
/// their `#id` (Perform Script, Go to Layout) lives inside the brackets instead.
fn split_inline_step_id(name_part: &str) -> (&str, Option<u32>) {
    let trimmed = name_part.trim_end();
    if let Some(hash) = trimmed.rfind(" #") {
        let digits = &trimmed[hash + 2..];
        if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
            if let Ok(id) = digits.parse::<u32>() {
                return (trimmed[..hash].trim_end(), Some(id));
            }
        }
    }
    (name_part, None)
}

/// Validate text and return **every** error found, not just the first — so the
/// editor can squiggle all problems at once. This is the linter behind the
/// `parse` JSON command and the VS Code diagnostics. It never touches the
/// clipboard and never bails early.
///
/// Catches:
///   - unknown / unsupported steps (no FileMaker ID in steps.toml)
///   - unclosed `[` brackets
///   - unbalanced blocks: `End If`/`End Loop`/`Else`/`Else If` with no open
///     block, `Exit Loop If` outside any loop, and `If`/`Loop` left unclosed.
///
/// Line tracking mirrors `parse_text_to_script` (same BOM/block-comment
/// preprocessing and the same depth-aware bracket scan) so the reported lines
/// line up with the source the user sees.
pub fn lint(text: &str) -> Vec<ParseError> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let preprocessed = preprocess_block_comments(text);
    let text = preprocessed.as_str();
    let lines: Vec<&str> = text.lines().collect();
    let mut errors: Vec<ParseError> = Vec::new();
    // (block-opener canonical name, 1-based line) for structure validation.
    let mut block_stack: Vec<(&'static str, usize)> = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        let trimmed = line.trim();
        // The `// ` disable marker is stripped only to read the step name —
        // disabled steps still participate in block structure (FM keeps them
        // nested), matching how the parser builds them.
        let content = if trimmed.starts_with("// ") { &trimmed[3..] } else { trimmed };
        if content.starts_with('#') {
            i += 1;
            continue;
        }

        let step_line = i; // 0-based; reported as +1
        let bracket_at = content.find(" [");
        let name_part = match bracket_at {
            Some(idx) => &content[..idx],
            None => content,
        };
        let (name_token, inline_id) = split_inline_step_id(name_part);
        let step_name = steps::translate_to_en(name_token);

        check_block(&step_name, step_line + 1, &mut block_stack, &mut errors);

        // Unknown steps are only an error when they lack an inline `#id` — with one
        // they're valid opaque-by-default steps that round-trip verbatim.
        if steps::id_for_en(&step_name).is_none() && inline_id.is_none() {
            errors.push(ParseError {
                line: step_line + 1,
                message: unknown_step_message(&step_name),
            });
        }

        if let Some(idx) = bracket_at {
            // Depth-aware scan to the unmatched `]`, identical to the parser, so
            // continuation lines of multi-line calcs aren't mistaken for steps.
            let mut depth: i32 = 0;
            let mut in_string = false;
            let mut terminated = false;
            let mut current_text = &content[idx + 2..];
            loop {
                for ch in current_text.chars() {
                    match ch {
                        '"' => in_string = !in_string,
                        '[' if !in_string => depth += 1,
                        ']' if !in_string => {
                            if depth == 0 {
                                terminated = true;
                                break;
                            }
                            depth -= 1;
                        }
                        _ => {}
                    }
                }
                if terminated {
                    break;
                }
                i += 1;
                if i >= lines.len() {
                    errors.push(ParseError {
                        line: step_line + 1,
                        message: format!("Unclosed `[` in step '{}'", step_name),
                    });
                    break;
                }
                current_text = lines[i];
            }
        }
        i += 1;
    }

    // Anything still open at EOF was never closed.
    for (name, line) in block_stack {
        errors.push(ParseError {
            line,
            message: format!("`{}` is never closed (missing `End {}`)", name, name),
        });
    }

    errors.sort_by_key(|e| e.line);
    errors
}

/// Apply one step's effect on the block stack, recording any imbalance.
fn check_block(
    name: &str,
    line: usize,
    stack: &mut Vec<(&'static str, usize)>,
    errors: &mut Vec<ParseError>,
) {
    let top = |s: &[(&'static str, usize)]| s.last().map(|(n, _)| *n);
    match name {
        "If" => stack.push(("If", line)),
        "Loop" => stack.push(("Loop", line)),
        "Else" | "Else If" => {
            if top(stack) != Some("If") {
                errors.push(ParseError {
                    line,
                    message: format!("`{}` without a matching `If`", name),
                });
            }
        }
        "End If" => {
            if top(stack) == Some("If") {
                stack.pop();
            } else {
                errors.push(ParseError {
                    line,
                    message: "`End If` without a matching `If`".to_string(),
                });
            }
        }
        "End Loop" => {
            if top(stack) == Some("Loop") {
                stack.pop();
            } else {
                errors.push(ParseError {
                    line,
                    message: "`End Loop` without a matching `Loop`".to_string(),
                });
            }
        }
        "Exit Loop If" => {
            if !stack.iter().any(|(n, _)| *n == "Loop") {
                errors.push(ParseError {
                    line,
                    message: "`Exit Loop If` outside of a `Loop`".to_string(),
                });
            }
        }
        _ => {}
    }
}

/// Build a ScriptStep from a name and optional bracket content.
/// Uses StepShape to determine which fields to populate.
fn build_step_from_name(name: &str, content: Option<&str>, enabled: bool, id: u32, indent: u32) -> ScriptStep {
    let shape = steps::shape_for_en(name);

    match shape {
        Some(StepShape::Comment) => ScriptStep {
            name: name.to_string(), enable: enabled, id,
            text: content.map(|c| c.to_string()),
            calculation: None, var_name: None, repetition: None,
            object_name: None, function_name: None, parameters: Vec::new(),
            restore_state: None, set_state: None,
            dialog_title: None, dialog_message: None, dialog_buttons: Vec::new(),
            field_result: None, field_target: None, field_table: None, field_numeric_id: None,
                script_target_name: None, script_target_id: None, current_script_mode: None,
                goto_location: None, goto_exit_after_last: None, goto_no_interact: None,
                window_mode: None, window_limit_current_file: None, window_state: None,
                layout_name: None, layout_id: None, layout_destination: None,
                window_height: None, window_width: None, window_top: None, window_left: None, window_style_name: None,
                find_requests: Vec::new(),
                curl_options: None, dont_encode_url: None, verify_ssl: None, select_all_state: None,
            indent_level: indent,
        },
        Some(StepShape::ValueCalcName) => {
            let (var_name, calculation) = parse_set_variable_content(content);
            ScriptStep {
                name: name.to_string(), enable: enabled, id,
                text: None, calculation, var_name, repetition: None,
                object_name: None, function_name: None, parameters: Vec::new(),
                restore_state: None, set_state: None,
                dialog_title: None, dialog_message: None, dialog_buttons: Vec::new(),
                field_result: None, field_target: None, field_table: None, field_numeric_id: None,
                script_target_name: None, script_target_id: None, current_script_mode: None,
                goto_location: None, goto_exit_after_last: None, goto_no_interact: None,
                window_mode: None, window_limit_current_file: None, window_state: None,
                layout_name: None, layout_id: None, layout_destination: None,
                window_height: None, window_width: None, window_top: None, window_left: None, window_style_name: None,
                find_requests: Vec::new(),
                curl_options: None, dont_encode_url: None, verify_ssl: None, select_all_state: None,
                indent_level: indent,
            }
        }
        Some(StepShape::CalculationWithRestore) => ScriptStep {
            name: name.to_string(), enable: enabled, id,
            text: None, calculation: content.map(|c| c.to_string()),
            var_name: None, repetition: None,
            object_name: None, function_name: None, parameters: Vec::new(),
            restore_state: Some("False".to_string()), set_state: None,
            dialog_title: None, dialog_message: None, dialog_buttons: Vec::new(),
            field_result: None, field_target: None, field_table: None, field_numeric_id: None,
                script_target_name: None, script_target_id: None, current_script_mode: None,
                goto_location: None, goto_exit_after_last: None, goto_no_interact: None,
                window_mode: None, window_limit_current_file: None, window_state: None,
                layout_name: None, layout_id: None, layout_destination: None,
                window_height: None, window_width: None, window_top: None, window_left: None, window_style_name: None,
                find_requests: Vec::new(),
                curl_options: None, dont_encode_url: None, verify_ssl: None, select_all_state: None,
            indent_level: indent,
        },
        Some(StepShape::SetState) => ScriptStep {
            name: name.to_string(), enable: enabled, id,
            text: None, calculation: None,
            var_name: None, repetition: None,
            object_name: None, function_name: None, parameters: Vec::new(),
            restore_state: None, set_state: content.map(|c| c.to_string()),
            dialog_title: None, dialog_message: None, dialog_buttons: Vec::new(),
            field_result: None, field_target: None, field_table: None, field_numeric_id: None,
                script_target_name: None, script_target_id: None, current_script_mode: None,
                goto_location: None, goto_exit_after_last: None, goto_no_interact: None,
                window_mode: None, window_limit_current_file: None, window_state: None,
                layout_name: None, layout_id: None, layout_destination: None,
                window_height: None, window_width: None, window_top: None, window_left: None, window_style_name: None,
                find_requests: Vec::new(),
                curl_options: None, dont_encode_url: None, verify_ssl: None, select_all_state: None,
            indent_level: indent,
        },
        Some(StepShape::Dialog) => {
            let (title, message, buttons) = parse_dialog_content(content);
            ScriptStep {
                name: name.to_string(), enable: enabled, id,
                text: None, calculation: None,
                var_name: None, repetition: None,
                object_name: None, function_name: None, parameters: Vec::new(),
                restore_state: None, set_state: None,
                dialog_title: title, dialog_message: message, dialog_buttons: buttons,
                field_result: None, field_target: None, field_table: None, field_numeric_id: None,
                script_target_name: None, script_target_id: None, current_script_mode: None,
                goto_location: None, goto_exit_after_last: None, goto_no_interact: None,
                window_mode: None, window_limit_current_file: None, window_state: None,
                layout_name: None, layout_id: None, layout_destination: None,
                window_height: None, window_width: None, window_top: None, window_left: None, window_style_name: None,
                find_requests: Vec::new(),
                curl_options: None, dont_encode_url: None, verify_ssl: None, select_all_state: None,
                indent_level: indent,
            }
        }
        Some(StepShape::FieldByName) => {
            let (result, target) = parse_field_content(content);
            ScriptStep {
                name: name.to_string(), enable: enabled, id,
                text: None, calculation: None,
                var_name: None, repetition: None,
                object_name: None, function_name: None, parameters: Vec::new(),
                restore_state: None, set_state: None,
                dialog_title: None, dialog_message: None, dialog_buttons: Vec::new(),
                field_result: result, field_target: target, field_table: None, field_numeric_id: None,
                script_target_name: None, script_target_id: None, current_script_mode: None,
                goto_location: None, goto_exit_after_last: None, goto_no_interact: None,
                window_mode: None, window_limit_current_file: None, window_state: None,
                layout_name: None, layout_id: None, layout_destination: None,
                window_height: None, window_width: None, window_top: None, window_left: None, window_style_name: None,
                find_requests: Vec::new(),
                curl_options: None, dont_encode_url: None, verify_ssl: None, select_all_state: None,
                indent_level: indent,
            }
        }
        Some(StepShape::SelectWindow) => {
            // Mode keywords are bare unquoted words. Anything else is a name calc.
            let modes = ["Current", "First", "Last", "Next", "Previous"];
            let (window_name, mode) = match content.map(|c| c.trim()) {
                Some(c) if modes.contains(&c) => (None, Some(c.to_string())),
                Some(c) if !c.is_empty()      => (Some(c.to_string()), Some("ByName".to_string())),
                _                             => (None, None),
            };
            ScriptStep {
                name: name.to_string(), enable: enabled, id,
                text: None, calculation: None,
                var_name: window_name, repetition: None,
                object_name: None, function_name: None, parameters: Vec::new(),
                restore_state: None, set_state: None,
                dialog_title: None, dialog_message: None, dialog_buttons: Vec::new(),
                field_result: None, field_target: None, field_table: None, field_numeric_id: None,
                script_target_name: None, script_target_id: None, current_script_mode: None,
                goto_location: None, goto_exit_after_last: None, goto_no_interact: None,
                window_mode: mode, window_limit_current_file: None, window_state: None,
                layout_name: None, layout_id: None, layout_destination: None,
                window_height: None, window_width: None, window_top: None, window_left: None, window_style_name: None,
                find_requests: Vec::new(),
                curl_options: None, dont_encode_url: None, verify_ssl: None, select_all_state: None,
                indent_level: indent,
            }
        }
        Some(StepShape::AdjustWindow) => {
            let state = content.map(|c| c.trim().to_string()).filter(|s| !s.is_empty());
            ScriptStep {
                name: name.to_string(), enable: enabled, id,
                text: None, calculation: None,
                var_name: None, repetition: None,
                object_name: None, function_name: None, parameters: Vec::new(),
                restore_state: None, set_state: None,
                dialog_title: None, dialog_message: None, dialog_buttons: Vec::new(),
                field_result: None, field_target: None, field_table: None, field_numeric_id: None,
                script_target_name: None, script_target_id: None, current_script_mode: None,
                goto_location: None, goto_exit_after_last: None, goto_no_interact: None,
                window_mode: None, window_limit_current_file: None, window_state: state,
                layout_name: None, layout_id: None, layout_destination: None,
                window_height: None, window_width: None, window_top: None, window_left: None, window_style_name: None,
                find_requests: Vec::new(),
                curl_options: None, dont_encode_url: None, verify_ssl: None, select_all_state: None,
                indent_level: indent,
            }
        }
        Some(StepShape::DataApi) => {
            let (target, calc) = parse_data_api_content(content);
            ScriptStep {
                name: name.to_string(), enable: enabled, id,
                text: None, calculation: calc,
                var_name: None, repetition: None,
                object_name: None, function_name: None, parameters: Vec::new(),
                restore_state: None, set_state: None,
                dialog_title: None, dialog_message: None, dialog_buttons: Vec::new(),
                field_result: None, field_target: target, field_table: None, field_numeric_id: None,
                script_target_name: None, script_target_id: None, current_script_mode: None,
                goto_location: None, goto_exit_after_last: None, goto_no_interact: None,
                window_mode: None, window_limit_current_file: None, window_state: None,
                layout_name: None, layout_id: None, layout_destination: None,
                window_height: None, window_width: None, window_top: None, window_left: None, window_style_name: None,
                find_requests: Vec::new(),
                curl_options: None, dont_encode_url: None, verify_ssl: None, select_all_state: None,
                indent_level: indent,
            }
        }
        Some(StepShape::GoToRecord) => {
            let (loc, exit, no_int, calc) = parse_goto_record_content(content);
            ScriptStep {
                name: name.to_string(), enable: enabled, id,
                text: None, calculation: calc,
                var_name: None, repetition: None,
                object_name: None, function_name: None, parameters: Vec::new(),
                restore_state: None, set_state: None,
                dialog_title: None, dialog_message: None, dialog_buttons: Vec::new(),
                field_result: None, field_target: None, field_table: None, field_numeric_id: None,
                script_target_name: None, script_target_id: None, current_script_mode: None,
                goto_location: loc, goto_exit_after_last: exit, goto_no_interact: no_int,
                window_mode: None, window_limit_current_file: None, window_state: None,
                layout_name: None, layout_id: None, layout_destination: None,
                window_height: None, window_width: None, window_top: None, window_left: None, window_style_name: None,
                find_requests: Vec::new(),
                curl_options: None, dont_encode_url: None, verify_ssl: None, select_all_state: None,
                indent_level: indent,
            }
        }
        Some(StepShape::PerformScript) => {
            let (script_name, script_id, calc) = parse_perform_script_content(content);
            ScriptStep {
                name: name.to_string(), enable: enabled, id,
                text: None, calculation: calc,
                var_name: None, repetition: None,
                object_name: None, function_name: None, parameters: Vec::new(),
                restore_state: None, set_state: None,
                dialog_title: None, dialog_message: None, dialog_buttons: Vec::new(),
                field_result: None, field_target: None, field_table: None, field_numeric_id: None,
                script_target_name: script_name, script_target_id: script_id, current_script_mode: None,
                goto_location: None, goto_exit_after_last: None, goto_no_interact: None,
                window_mode: None, window_limit_current_file: None, window_state: None,
                layout_name: None, layout_id: None, layout_destination: None,
                window_height: None, window_width: None, window_top: None, window_left: None, window_style_name: None,
                find_requests: Vec::new(),
                curl_options: None, dont_encode_url: None, verify_ssl: None, select_all_state: None,
                indent_level: indent,
            }
        }
        Some(StepShape::FieldAndCalc) => {
            let (table, target, numeric_id, calc) = parse_field_and_calc_content(content);
            ScriptStep {
                name: name.to_string(), enable: enabled, id,
                text: None, calculation: calc,
                var_name: None, repetition: None,
                object_name: None, function_name: None, parameters: Vec::new(),
                restore_state: None, set_state: None,
                dialog_title: None, dialog_message: None, dialog_buttons: Vec::new(),
                field_result: None, field_target: target, field_table: table, field_numeric_id: numeric_id,
                script_target_name: None, script_target_id: None, current_script_mode: None,
                goto_location: None, goto_exit_after_last: None, goto_no_interact: None,
                window_mode: None, window_limit_current_file: None, window_state: None,
                layout_name: None, layout_id: None, layout_destination: None,
                window_height: None, window_width: None, window_top: None, window_left: None, window_style_name: None,
                find_requests: Vec::new(),
                curl_options: None, dont_encode_url: None, verify_ssl: None, select_all_state: None,
                indent_level: indent,
            }
        }
        Some(StepShape::ReplaceFieldContents) => {
            let (table, target, calc, dialog_off) = parse_replace_field_contents_content(content);
            ScriptStep {
                name: name.to_string(), enable: enabled, id,
                text: None, calculation: calc,
                var_name: None, repetition: None,
                object_name: None, function_name: None, parameters: Vec::new(),
                restore_state: None, set_state: None,
                dialog_title: None, dialog_message: None, dialog_buttons: Vec::new(),
                field_result: None, field_target: target, field_table: table, field_numeric_id: None,
                script_target_name: None, script_target_id: None, current_script_mode: None,
                goto_location: None, goto_exit_after_last: None,
                goto_no_interact: if dialog_off { Some("True".to_string()) } else { None },
                window_mode: None, window_limit_current_file: None, window_state: None,
                layout_name: None, layout_id: None, layout_destination: None,
                window_height: None, window_width: None, window_top: None, window_left: None, window_style_name: None,
                find_requests: Vec::new(),
                curl_options: None, dont_encode_url: None, verify_ssl: None, select_all_state: None,
                indent_level: indent,
            }
        }
        Some(StepShape::WebViewerJs) => {
            let (obj, func, params) = parse_js_content(content);
            ScriptStep {
                name: name.to_string(), enable: enabled, id,
                text: None, calculation: None,
                var_name: None, repetition: None,
                object_name: obj, function_name: func, parameters: params,
                restore_state: None, set_state: None,
                dialog_title: None, dialog_message: None, dialog_buttons: Vec::new(),
                field_result: None, field_target: None, field_table: None, field_numeric_id: None,
                script_target_name: None, script_target_id: None, current_script_mode: None,
                goto_location: None, goto_exit_after_last: None, goto_no_interact: None,
                window_mode: None, window_limit_current_file: None, window_state: None,
                layout_name: None, layout_id: None, layout_destination: None,
                window_height: None, window_width: None, window_top: None, window_left: None, window_style_name: None,
                find_requests: Vec::new(),
                curl_options: None, dont_encode_url: None, verify_ssl: None, select_all_state: None,
                indent_level: indent,
            }
        }
        Some(StepShape::GoToObject) => {
            let (obj, rep) = parse_go_to_object_content(content);
            ScriptStep {
                name: name.to_string(), enable: enabled, id,
                text: None, calculation: None,
                var_name: None, repetition: rep,
                object_name: obj, function_name: None, parameters: Vec::new(),
                restore_state: None, set_state: None,
                dialog_title: None, dialog_message: None, dialog_buttons: Vec::new(),
                field_result: None, field_target: None, field_table: None, field_numeric_id: None,
                script_target_name: None, script_target_id: None, current_script_mode: None,
                goto_location: None, goto_exit_after_last: None, goto_no_interact: None,
                window_mode: None, window_limit_current_file: None, window_state: None,
                layout_name: None, layout_id: None, layout_destination: None,
                window_height: None, window_width: None, window_top: None, window_left: None, window_style_name: None,
                find_requests: Vec::new(),
                curl_options: None, dont_encode_url: None, verify_ssl: None, select_all_state: None,
                indent_level: indent,
            }
        }
        Some(StepShape::GoToLayoutNamed) => {
            let (layout, layout_id, dest) = parse_go_to_layout_content(content);
            ScriptStep {
                name: name.to_string(), enable: enabled, id,
                text: None, calculation: None,
                var_name: None, repetition: None,
                object_name: None, function_name: None, parameters: Vec::new(),
                restore_state: None, set_state: None,
                dialog_title: None, dialog_message: None, dialog_buttons: Vec::new(),
                field_result: None, field_target: None, field_table: None, field_numeric_id: None,
                script_target_name: None, script_target_id: None, current_script_mode: None,
                goto_location: None, goto_exit_after_last: None, goto_no_interact: None,
                window_mode: None, window_limit_current_file: None, window_state: None,
                layout_name: layout, layout_id, layout_destination: dest,
                window_height: None, window_width: None, window_top: None, window_left: None, window_style_name: None,
                find_requests: Vec::new(),
                curl_options: None, dont_encode_url: None, verify_ssl: None, select_all_state: None,
                indent_level: indent,
            }
        }
        Some(StepShape::NewWindow) => {
            let nw = parse_new_window_content(content);
            ScriptStep {
                name: name.to_string(), enable: enabled, id,
                text: None, calculation: None,
                var_name: None, repetition: None,
                object_name: None, function_name: None, parameters: Vec::new(),
                restore_state: None, set_state: None,
                dialog_title: None, dialog_message: None, dialog_buttons: Vec::new(),
                field_result: None, field_target: None, field_table: None, field_numeric_id: None,
                script_target_name: None, script_target_id: None, current_script_mode: None,
                goto_location: None, goto_exit_after_last: None, goto_no_interact: None,
                window_mode: None, window_limit_current_file: None, window_state: None,
                layout_name: nw.layout, layout_id: None, layout_destination: None,
                window_height: nw.height, window_width: nw.width, window_top: nw.top, window_left: nw.left,
                window_style_name: nw.style,
                find_requests: Vec::new(),
                curl_options: None, dont_encode_url: None, verify_ssl: None, select_all_state: None,
                indent_level: indent,
            }
        }
        Some(StepShape::InsertFromUrl) => {
            let p = parse_insert_from_url_content(content);
            ScriptStep {
                name: name.to_string(), enable: enabled, id,
                text: None, calculation: p.url,
                var_name: None, repetition: None,
                object_name: None, function_name: None, parameters: Vec::new(),
                restore_state: None, set_state: None,
                dialog_title: None, dialog_message: None, dialog_buttons: Vec::new(),
                field_result: None, field_target: p.target, field_table: p.table, field_numeric_id: None,
                script_target_name: None, script_target_id: None, current_script_mode: None,
                goto_location: None, goto_exit_after_last: None,
                goto_no_interact: if p.dialog_off { Some("True".to_string()) } else { None },
                window_mode: None, window_limit_current_file: None, window_state: None,
                layout_name: None, layout_id: None, layout_destination: None,
                window_height: None, window_width: None, window_top: None, window_left: None, window_style_name: None,
                find_requests: Vec::new(),
                curl_options: p.curl,
                dont_encode_url: if p.dont_encode { Some("True".to_string()) } else { None },
                verify_ssl: if p.verify_ssl { Some("True".to_string()) } else { None },
                select_all_state: if p.select_all { Some("True".to_string()) } else { None },
                indent_level: indent,
            }
        }
        Some(StepShape::PerformFind) => {
            let requests = parse_perform_find_content(content);
            ScriptStep {
                name: name.to_string(), enable: enabled, id,
                text: None, calculation: None,
                var_name: None, repetition: None,
                object_name: None, function_name: None, parameters: Vec::new(),
                restore_state: None, set_state: None,
                dialog_title: None, dialog_message: None, dialog_buttons: Vec::new(),
                field_result: None, field_target: None, field_table: None, field_numeric_id: None,
                script_target_name: None, script_target_id: None, current_script_mode: None,
                goto_location: None, goto_exit_after_last: None, goto_no_interact: None,
                window_mode: None, window_limit_current_file: None, window_state: None,
                layout_name: None, layout_id: None, layout_destination: None,
                window_height: None, window_width: None, window_top: None, window_left: None, window_style_name: None,
                find_requests: requests,
                curl_options: None, dont_encode_url: None, verify_ssl: None, select_all_state: None,
                indent_level: indent,
            }
        }
        // Calculation, Plain, Opaque, or unknown — store content as calculation.
        // Opaque keeps the bracket content (raw inner FM XML) verbatim.
        Some(StepShape::Calculation) | Some(StepShape::Plain) | Some(StepShape::Opaque) | None => ScriptStep {
            name: name.to_string(), enable: enabled, id,
            text: None, calculation: content.map(|c| c.to_string()),
            var_name: None, repetition: None,
            object_name: None, function_name: None, parameters: Vec::new(),
            restore_state: None, set_state: None,
            dialog_title: None, dialog_message: None, dialog_buttons: Vec::new(),
            field_result: None, field_target: None, field_table: None, field_numeric_id: None,
                script_target_name: None, script_target_id: None, current_script_mode: None,
                goto_location: None, goto_exit_after_last: None, goto_no_interact: None,
                window_mode: None, window_limit_current_file: None, window_state: None,
                layout_name: None, layout_id: None, layout_destination: None,
                window_height: None, window_width: None, window_top: None, window_left: None, window_style_name: None,
                find_requests: Vec::new(),
                curl_options: None, dont_encode_url: None, verify_ssl: None, select_all_state: None,
            indent_level: indent,
        },
    }
}

// ─── Content parsers ───

/// Parse "Set Variable" bracket content: `$var = calculation`
fn parse_set_variable_content(content: Option<&str>) -> (Option<String>, Option<String>) {
    let content = match content {
        Some(c) => c,
        None => return (None, None),
    };

    if let Some(eq_idx) = content.find(" = ") {
        (Some(content[..eq_idx].trim().to_string()), Some(content[eq_idx + 3..].trim().to_string()))
    } else {
        (None, Some(content.to_string()))
    }
}

/// Parse "Show Custom Dialog" bracket content: `Title: ...; Message: ...; Buttons: ...`
/// Uses bracket-aware splitting so semicolons inside calculations don't break parsing.
fn parse_dialog_content(content: Option<&str>) -> (Option<String>, Option<String>, Vec<String>) {
    let content = match content {
        Some(c) => c,
        None => return (None, None, Vec::new()),
    };

    let mut title = None;
    let mut message = None;
    let mut buttons = Vec::new();

    for part in split_smart(content) {
        let part = part.trim();
        if part.starts_with("Title: ") {
            title = Some(part[7..].to_string());
        } else if part.starts_with("Message: ") {
            message = Some(part[9..].to_string());
        } else if part.starts_with("Buttons: ") {
            buttons = part[9..].split(',').map(|b| b.trim().to_string()).collect();
        }
    }

    (title, message, buttons)
}

/// Parse Execute FileMaker Data API content: `$target; query_calc` or just `query_calc`.
/// Splits on the first `;` at bracket depth 0 (so semicolons inside the JSON don't trigger).
fn parse_data_api_content(content: Option<&str>) -> (Option<String>, Option<String>) {
    let content = match content {
        Some(c) => c.trim(),
        None => return (None, None),
    };

    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut split_at: Option<usize> = None;
    for (byte_pos, ch) in content.char_indices() {
        match ch {
            '"' => in_string = !in_string,
            '[' | '(' if !in_string => depth += 1,
            ']' | ')' if !in_string => depth -= 1,
            ';' if !in_string && depth == 0 => { split_at = Some(byte_pos); break; }
            _ => {}
        }
    }

    match split_at {
        Some(pos) => {
            let target = content[..pos].trim().to_string();
            let calc = content[pos + 1..].trim().to_string();
            (
                if target.is_empty() { None } else { Some(target) },
                if calc.is_empty() { None } else { Some(calc) },
            )
        }
        None => (None, Some(content.to_string())),
    }
}

/// Parse Go to Record/Request/Page content: `[First|Last|Next|Previous|Calc: expr]; [Exit]; [NoInteract]`
/// Returns (location, exit_after_last, no_interact, calculation).
fn parse_goto_record_content(content: Option<&str>) -> (Option<String>, Option<String>, Option<String>, Option<String>) {
    let content = match content {
        Some(c) => c.trim(),
        None => return (None, None, None, None),
    };
    if content.is_empty() {
        return (None, None, None, None);
    }

    let mut location: Option<String> = None;
    let mut exit_flag: Option<String> = None;
    let mut no_interact: Option<String> = None;
    let mut calc: Option<String> = None;

    for part in split_smart(content) {
        let token = part.trim();
        match token {
            "First" | "Last" | "Next" | "Previous" => location = Some(token.to_string()),
            "Exit" => exit_flag = Some("True".to_string()),
            "NoInteract" => no_interact = Some("True".to_string()),
            _ if token.starts_with("Calc:") => {
                location = Some("byCalculation".to_string());
                calc = Some(token[5..].trim().to_string());
            }
            _ => {}
        }
    }

    (location, exit_flag, no_interact, calc)
}

/// Parse Perform Script content. Recognized forms:
///   `"ScriptName"`           → script only, no param
///   `"ScriptName"; param`    → script + param
///   `param`                  → param only (legacy, when no script target was set)
/// The script name is detected by a leading `"` and closes at the matching `"`.
fn parse_perform_script_content(content: Option<&str>) -> (Option<String>, Option<String>, Option<String>) {
    let content = match content {
        Some(c) => c.trim(),
        None => return (None, None, None),
    };

    if !content.starts_with('"') {
        // No script target — entire content is the parameter calc.
        return (None, None, Some(content.to_string()));
    }

    let after_open = &content[1..];
    let close_pos = match after_open.find('"') {
        Some(p) => p,
        None => return (None, None, Some(content.to_string())),
    };
    let script_name = after_open[..close_pos].to_string();
    let rest = after_open[close_pos + 1..].trim_start();

    // Optional `#N` id suffix — required by FM to resolve the script link on paste.
    let (script_id, rest) = if let Some(after_hash) = rest.strip_prefix('#') {
        let after_hash = after_hash.trim_start();
        let end = after_hash.find(|c: char| !c.is_ascii_digit()).unwrap_or(after_hash.len());
        if end > 0 {
            (Some(after_hash[..end].to_string()), after_hash[end..].trim_start())
        } else {
            (None, rest)
        }
    } else {
        (None, rest)
    };

    let calc = if let Some(stripped) = rest.strip_prefix(';') {
        let s = stripped.trim();
        if s.is_empty() { None } else { Some(s.to_string()) }
    } else if rest.is_empty() {
        None
    } else {
        Some(rest.to_string())
    };

    (Some(script_name), script_id, calc)
}

/// Parse Set Field content: `Table::Field #id; calc` or any subset.
/// Returns (table, field_name, numeric_id, calc). The `#id` suffix is optional;
/// when omitted, FM resolves the field by name on paste.
fn parse_field_and_calc_content(content: Option<&str>) -> (Option<String>, Option<String>, Option<String>, Option<String>) {
    let content = match content {
        Some(c) => c,
        None => return (None, None, None, None),
    };

    // Find the first ';' at depth 0 (outside any [ ] or "..." pair).
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut split_at: Option<usize> = None;
    for (byte_pos, ch) in content.char_indices() {
        match ch {
            '"' => in_string = !in_string,
            '[' | '(' if !in_string => depth += 1,
            ']' | ')' if !in_string => depth -= 1,
            ';' if !in_string && depth == 0 => { split_at = Some(byte_pos); break; }
            _ => {}
        }
    }

    let (target_str, calc_str) = match split_at {
        Some(pos) => (content[..pos].trim().to_string(), content[pos + 1..].trim().to_string()),
        None => return (None, None, None, Some(content.trim().to_string())),
    };

    // Strip optional ` #N` suffix from the target (numeric FM field id).
    let (target_str, numeric_id) = match target_str.rfind(" #") {
        Some(idx) => {
            let after = &target_str[idx + 2..];
            if !after.is_empty() && after.chars().all(|c| c.is_ascii_digit()) {
                (target_str[..idx].trim().to_string(), Some(after.to_string()))
            } else {
                (target_str, None)
            }
        }
        None => (target_str, None),
    };

    let (table, name) = if let Some(idx) = target_str.find("::") {
        (Some(target_str[..idx].to_string()), Some(target_str[idx + 2..].to_string()))
    } else if target_str.is_empty() {
        (None, None)
    } else {
        (None, Some(target_str))
    };

    let calc = if calc_str.is_empty() { None } else { Some(calc_str) };
    (table, name, numeric_id, calc)
}

/// Parse "Replace Field Contents" bracket content: `Table::Field; calc[; Dialog: Off]`.
/// Returns (table, field, calc, dialog_off). The first `;`-segment (bracket-aware)
/// is the target; a segment equal to `Dialog: Off` toggles the flag; the rest is the
/// calc (re-joined with `; ` in case the calc itself contained a top-level `;`).
fn parse_replace_field_contents_content(content: Option<&str>) -> (Option<String>, Option<String>, Option<String>, bool) {
    let content = match content {
        Some(c) => c.trim(),
        None => return (None, None, None, false),
    };
    if content.is_empty() {
        return (None, None, None, false);
    }

    // Quote- and bracket-aware split on `;` so a `;` inside a string ("a;b") or
    // inside a calc's ()/[] doesn't terminate a segment.
    let mut segments: Vec<String> = Vec::new();
    {
        let mut cur = String::new();
        let mut depth: i32 = 0;
        let mut in_string = false;
        for ch in content.chars() {
            match ch {
                '"' => { in_string = !in_string; cur.push(ch); }
                '[' | '(' if !in_string => { depth += 1; cur.push(ch); }
                ']' | ')' if !in_string => { depth -= 1; cur.push(ch); }
                ';' if !in_string && depth == 0 => { segments.push(cur.trim().to_string()); cur.clear(); }
                _ => cur.push(ch),
            }
        }
        if !cur.trim().is_empty() {
            segments.push(cur.trim().to_string());
        }
    }
    if segments.is_empty() {
        return (None, None, None, false);
    }

    // First segment is the target field. Strip an optional ` #N` numeric id suffix
    // (we never emit it, but tolerate it if hand-typed).
    let mut target_str = segments[0].trim().to_string();
    if let Some(idx) = target_str.rfind(" #") {
        let after = &target_str[idx + 2..];
        if !after.is_empty() && after.chars().all(|c| c.is_ascii_digit()) {
            target_str = target_str[..idx].trim().to_string();
        }
    }
    let (table, field) = if let Some(idx) = target_str.find("::") {
        (Some(target_str[..idx].to_string()), Some(target_str[idx + 2..].to_string()))
    } else if target_str.is_empty() {
        (None, None)
    } else {
        (None, Some(target_str))
    };

    let mut dialog_off = false;
    let mut calc_parts: Vec<String> = Vec::new();
    for seg in segments.iter().skip(1) {
        if seg.eq_ignore_ascii_case("Dialog: Off") {
            dialog_off = true;
        } else {
            calc_parts.push(seg.clone());
        }
    }
    let calc = if calc_parts.is_empty() { None } else { Some(calc_parts.join("; ")) };

    (table, field, calc, dialog_off)
}

/// Parse "Set Field By Name" bracket content: `Result: ...; Target: ...`
fn parse_field_content(content: Option<&str>) -> (Option<String>, Option<String>) {
    let content = match content {
        Some(c) => c,
        None => return (None, None),
    };

    let mut result = None;
    let mut target = None;

    for part in split_smart(content) {
        let part = part.trim();
        if part.starts_with("Result: ") {
            result = Some(part[8..].to_string());
        } else if part.starts_with("Target: ") {
            target = Some(part[8..].to_string());
        }
    }

    (result, target)
}

/// Parse "Perform JavaScript in Web Viewer" bracket content: `Object: ...; Function: ...; Param[0]: ...`
fn parse_js_content(content: Option<&str>) -> (Option<String>, Option<String>, Vec<String>) {
    let content = match content {
        Some(c) => c,
        None => return (None, None, Vec::new()),
    };

    let mut object_name = None;
    let mut function_name = None;
    let mut parameters = Vec::new();

    for part in split_smart(content) {
        let part = part.trim();
        if part.starts_with("Object: ") {
            object_name = Some(part[8..].to_string());
        } else if part.starts_with("Function: ") {
            function_name = Some(part[10..].to_string());
        } else if part.starts_with("Param[") {
            if let Some(colon_idx) = part.find("]: ") {
                parameters.push(part[colon_idx + 3..].to_string());
            }
        }
    }

    (object_name, function_name, parameters)
}

/// Parse `Go to Object` bracket content: `"objectName"` or `"objectName"; Rep: N`.
/// Quotes are optional. Returns (object_name_calc, repetition_calc).
fn parse_go_to_object_content(content: Option<&str>) -> (Option<String>, Option<String>) {
    let content = match content { Some(c) => c.trim(), None => return (None, None) };
    if content.is_empty() { return (None, None); }
    let mut obj: Option<String> = None;
    let mut rep: Option<String> = None;
    for part in split_smart(content) {
        let p = part.trim();
        if let Some(v) = p.strip_prefix("Rep:") {
            rep = Some(v.trim().to_string());
        } else if obj.is_none() {
            obj = Some(p.to_string());
        }
    }
    (obj, rep)
}

/// Parse `Go to Layout` bracket content. Forms accepted:
///   `original`         → OriginalLayout
///   `"Name"`           → SelectedLayout, no id (FM may fail to link on paste)
///   `"Name" #N`        → SelectedLayout with FM Layout id N (round-trip exact)
/// Returns (layout_name, layout_id, destination).
fn parse_go_to_layout_content(content: Option<&str>) -> (Option<String>, Option<String>, Option<String>) {
    let content = match content { Some(c) => c.trim(), None => return (None, None, None) };
    if content.eq_ignore_ascii_case("original") {
        return (None, None, Some("OriginalLayout".to_string()));
    }
    // Split off optional ` #N` numeric id suffix.
    let (name_part, id) = match content.rfind(" #") {
        Some(idx) => {
            let after = &content[idx + 2..];
            if !after.is_empty() && after.chars().all(|c| c.is_ascii_digit()) {
                (content[..idx].trim().to_string(), Some(after.to_string()))
            } else {
                (content.to_string(), None)
            }
        }
        None => (content.to_string(), None),
    };
    let name = name_part.trim_matches('"').to_string();
    if name.is_empty() {
        (None, id, None)
    } else {
        (Some(name), id, Some("SelectedLayout".to_string()))
    }
}

/// Parsed bag of New Window fields — used to pass through `build_step_from_name`.
struct ParsedNewWindow {
    style: Option<String>,
    layout: Option<String>,
    height: Option<String>,
    width: Option<String>,
    top: Option<String>,
    left: Option<String>,
}

/// Parse `New Window` bracket content. Key/value pairs separated by `;`:
/// `Style: Document; Layout: "Name"; Height: 1; Width: 1; Top: -1000; Left: -1000`.
fn parse_new_window_content(content: Option<&str>) -> ParsedNewWindow {
    let mut out = ParsedNewWindow { style: None, layout: None, height: None, width: None, top: None, left: None };
    let content = match content { Some(c) => c, None => return out };
    for part in split_smart(content) {
        let p = part.trim();
        if let Some(v) = p.strip_prefix("Style:")     { out.style  = Some(v.trim().to_string()); }
        else if let Some(v) = p.strip_prefix("Layout:") { out.layout = Some(v.trim().trim_matches('"').to_string()); }
        else if let Some(v) = p.strip_prefix("Height:") { out.height = Some(v.trim().to_string()); }
        else if let Some(v) = p.strip_prefix("Width:")  { out.width  = Some(v.trim().to_string()); }
        else if let Some(v) = p.strip_prefix("Top:")    { out.top    = Some(v.trim().to_string()); }
        else if let Some(v) = p.strip_prefix("Left:")   { out.left   = Some(v.trim().to_string()); }
    }
    out
}

/// Parsed bag of Insert from URL fields. Filled by `parse_insert_from_url_content`.
struct ParsedInsertFromUrl {
    target: Option<String>,
    table: Option<String>,
    url: Option<String>,
    curl: Option<String>,
    dialog_off: bool,
    verify_ssl: bool,
    select_all: bool,
    dont_encode: bool,
}

/// Parse `Insert from URL` bracket content. Key/value pairs separated by `;`:
///   `Target: $var | Table::Field`
///   `URL: <calc expression>`
///   `cURL: <calc expression>`
///   bare flags (any order): `Dialog: Off`, `VerifySSL`, `SelectAll`, `DontEncode`
fn parse_insert_from_url_content(content: Option<&str>) -> ParsedInsertFromUrl {
    let mut out = ParsedInsertFromUrl {
        target: None, table: None, url: None, curl: None,
        dialog_off: false, verify_ssl: false, select_all: false, dont_encode: false,
    };
    let content = match content { Some(c) => c, None => return out };
    for part in split_smart(content) {
        let p = part.trim();
        if let Some(v) = p.strip_prefix("Target:") {
            let tgt = v.trim();
            if let Some(idx) = tgt.find("::") {
                out.table = Some(tgt[..idx].to_string());
                out.target = Some(tgt[idx + 2..].to_string());
            } else {
                out.target = Some(tgt.to_string());
            }
        } else if let Some(v) = p.strip_prefix("URL:")  { out.url  = Some(v.trim().to_string()); }
          else if let Some(v) = p.strip_prefix("cURL:") { out.curl = Some(v.trim().to_string()); }
          else if let Some(v) = p.strip_prefix("Dialog:") { if v.trim().eq_ignore_ascii_case("off") { out.dialog_off = true; } }
          else if p.eq_ignore_ascii_case("VerifySSL")  { out.verify_ssl = true; }
          else if p.eq_ignore_ascii_case("SelectAll")  { out.select_all = true; }
          else if p.eq_ignore_ascii_case("DontEncode") { out.dont_encode = true; }
    }
    out
}

/// Parse `Perform Find` bracket content. Multi-line DSL:
///
/// ```text
/// Find: T::F1 => value1; T::F2 => value2
/// Omit: T::F3 => value3
/// ```
///
/// Each `Find:` / `Omit:` opens one RequestRow. Criteria within a row are
/// `;`-separated, and each criterion is `Table::Field => value`.
fn parse_perform_find_content(content: Option<&str>) -> Vec<crate::xmss::FindRequest> {
    use crate::xmss::{FindCriterion, FindRequest};
    let content = match content { Some(c) => c, None => return Vec::new() };
    let mut requests: Vec<FindRequest> = Vec::new();
    // Split on logical lines (newlines), then within a line look for the `Find:` /
    // `Omit:` header. Bracket-aware splitting is overkill here — the DSL is intentionally
    // flat, and any complex value would have been authored on a single criterion.
    for raw in content.split('\n') {
        let line = raw.trim();
        if line.is_empty() { continue; }
        let (op, rest) = if let Some(r) = line.strip_prefix("Find:") {
            ("Include", r.trim())
        } else if let Some(r) = line.strip_prefix("Omit:") {
            ("Omit", r.trim())
        } else {
            continue; // ignore stray content
        };
        let mut req = FindRequest { operation: op.to_string(), criteria: Vec::new() };
        for crit_str in rest.split(';') {
            let cs = crit_str.trim();
            if cs.is_empty() { continue; }
            let (target, value) = match cs.find("=>") {
                Some(idx) => (cs[..idx].trim().to_string(), cs[idx + 2..].trim().to_string()),
                None => continue,
            };
            let (table, field) = match target.find("::") {
                Some(idx) => (target[..idx].to_string(), target[idx + 2..].to_string()),
                None => (String::new(), target),
            };
            req.criteria.push(FindCriterion { table, field, text: value });
        }
        requests.push(req);
    }
    requests
}

/// Split content by `;` but respect brackets `[]` and parentheses `()`.
/// This prevents splitting on semicolons inside FileMaker calculations.
fn split_smart(content: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0;

    for ch in content.chars() {
        match ch {
            '[' | '(' => { depth += 1; current.push(ch); }
            ']' | ')' => { depth -= 1; current.push(ch); }
            ';' if depth == 0 => {
                if !current.trim().is_empty() {
                    parts.push(current.trim().to_string());
                }
                current = String::new();
            }
            _ => { current.push(ch); }
        }
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }

    parts
}

#[cfg(test)]
mod tests {
    use crate::xmss;

    // Real capture from FileMaker (debug_raw.xml). Replace Field Contents carries
    // a dialog flag, replace mode, serial-number options and a target field — all
    // of which the old `None` shape dropped. Opaque must round-trip it verbatim.
    const REPLACE_FIELD_CONTENTS: &str = "<fmxmlsnippet type=\"FMObjectList\"><Step enable=\"True\" id=\"91\" name=\"Replace Field Contents\"><NoInteract state=\"True\"></NoInteract><Restore state=\"True\"></Restore><With value=\"Calculation\"></With><Calculation><![CDATA[\"pruebas\"]]></Calculation><SerialNumbers PerformAutoEnter=\"True\" UpdateEntryOptions=\"False\" UseEntryOptions=\"True\"></SerialNumbers><Field table=\"Cli_d_Sesiones\" id=\"1509\" name=\"g__END__\"></Field></Step></fmxmlsnippet>";

    #[test]
    fn replace_field_contents_decodes_to_structured_text() {
        let script = xmss::parse_fmxml_snippet(REPLACE_FIELD_CONTENTS).unwrap();
        let s = &script.steps[0];
        assert_eq!(s.field_table.as_deref(), Some("Cli_d_Sesiones"));
        assert_eq!(s.field_target.as_deref(), Some("g__END__"));
        assert_eq!(s.calculation.as_deref(), Some("\"pruebas\""));
        assert_eq!(s.goto_no_interact.as_deref(), Some("True")); // dialog off

        let text = super::format_script(&script);
        assert_eq!(text, "Replace Field Contents [Cli_d_Sesiones::g__END__; \"pruebas\"; Dialog: Off]");
    }

    #[test]
    fn replace_field_contents_roundtrips() {
        // Round-trip reproduces the original XML, minus the Field `id` (we resolve
        // by name on paste, same as Set Field).
        let script = xmss::parse_fmxml_snippet(REPLACE_FIELD_CONTENTS).unwrap();
        let text = super::format_script(&script);
        let script2 = super::parse_text_to_script(&text).unwrap();
        let rebuilt = xmss::build_xml_from_script(&script2).unwrap();
        let expected = REPLACE_FIELD_CONTENTS.replace(" id=\"1509\"", "");
        assert_eq!(rebuilt, expected);
    }

    #[test]
    fn replace_field_contents_authored_from_scratch() {
        // Hand-authored (Spanish name, no dialog flag → dialog stays on).
        let script = super::parse_text_to_script(
            "Reemplazar contenido del campo [Ta_d_MovimientosRef::MovRef_Del; 1]",
        ).unwrap();
        let s = &script.steps[0];
        assert_eq!(s.name, "Replace Field Contents");
        let xml = xmss::build_xml_from_script(&script).unwrap();
        assert!(xml.contains("<Field table=\"Ta_d_MovimientosRef\" name=\"MovRef_Del\"></Field>"));
        assert!(xml.contains("<NoInteract state=\"False\">")); // dialog on (default)
        assert!(xml.contains("<Calculation><![CDATA[1]]></Calculation>"));
        assert!(xml.contains("<With value=\"Calculation\">"));
    }

    #[test]
    fn replace_field_contents_spanish_name_translates() {
        let es = REPLACE_FIELD_CONTENTS.replace(
            "name=\"Replace Field Contents\"",
            "name=\"Reemplazar contenido del campo\"",
        );
        let script = xmss::parse_fmxml_snippet(&es).unwrap();
        assert_eq!(script.steps[0].name, "Replace Field Contents");
    }

    #[test]
    fn write_accepts_spanish_step_name_with_brackets() {
        // Authoring a .fmscript using the Spanish step name must resolve to the
        // English canonical step (and its FM id) just like an English name.
        let script = super::parse_text_to_script("Establecer variable [$x = 1]").unwrap();
        assert_eq!(script.steps[0].name, "Set Variable");
        assert_eq!(script.steps[0].var_name.as_deref(), Some("$x"));
    }

    #[test]
    fn write_accepts_spanish_step_name_without_brackets() {
        let script = super::parse_text_to_script("Fin Si").unwrap();
        assert_eq!(script.steps[0].name, "End If");
    }

    #[test]
    fn parse_error_reports_line_for_unknown_step() {
        let text = "Set Variable [$x = 1]\nShow All Records\nNoSuchStep [foo]";
        let err = super::parse_text_to_script(text).unwrap_err();
        assert_eq!(err.line, 3);
        assert!(err.message.contains("NoSuchStep"));
        assert_eq!(err.to_string(), format!("Line 3: {}", err.message));
    }

    #[test]
    fn parse_error_reports_line_for_unclosed_bracket() {
        // `[` opens on line 2 and never closes.
        let text = "Set Variable [$x = 1]\nSet Variable [$y = oops";
        let err = super::parse_text_to_script(text).unwrap_err();
        assert_eq!(err.line, 2);
        assert!(err.message.contains("Unclosed"));
    }

    #[test]
    fn lint_accepts_a_valid_script() {
        let text = "Set Variable [$x = 1]\nIf [$x = 1]\n  Show All Records\nEnd If";
        assert!(super::lint(text).is_empty());
    }

    #[test]
    fn lint_reports_all_errors_at_once() {
        // Two independent unknown steps on lines 1 and 3 — both must be reported,
        // not just the first (unlike parse_text_to_script which bails).
        let text = "BogusOne [a]\nShow All Records\nBogusTwo [b]";
        let errs = super::lint(text);
        assert_eq!(errs.len(), 2);
        assert_eq!(errs[0].line, 1);
        assert_eq!(errs[1].line, 3);
    }

    #[test]
    fn lint_flags_unclosed_if() {
        let text = "If [$x = 1]\n  Show All Records";
        let errs = super::lint(text);
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].line, 1);
        assert!(errs[0].message.contains("never closed"));
    }

    #[test]
    fn lint_flags_orphan_end_if() {
        let text = "Show All Records\nEnd If";
        let errs = super::lint(text);
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].line, 2);
        assert!(errs[0].message.contains("End If"));
    }

    #[test]
    fn lint_flags_else_without_if() {
        let text = "Show All Records\nElse\n  Halt Script";
        let errs = super::lint(text);
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].line, 2);
        assert!(errs[0].message.contains("Else"));
    }

    #[test]
    fn lint_flags_exit_loop_if_outside_loop() {
        let text = "Exit Loop If [$x = 1]";
        let errs = super::lint(text);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].message.contains("Exit Loop If"));
    }

    #[test]
    fn lint_accepts_nested_blocks_and_else() {
        let text = "Loop\n  If [$x = 1]\n    Exit Loop If [$y = 2]\n  Else\n    Show All Records\n  End If\nEnd Loop";
        assert!(super::lint(text).is_empty());
    }

    #[test]
    fn lint_suggests_correct_casing() {
        // Lowercase `if` and a spaced/cased slip both get a "did you mean" hint.
        let errs = super::lint("if [$x = 1]\nEnd If");
        assert!(errs.iter().any(|e| e.message.contains("did you mean 'If'")));

        let errs = super::lint("setvariable [ $x ; Value: 1 ]");
        assert!(errs
            .iter()
            .any(|e| e.message.contains("did you mean 'Set Variable'")));
    }

    #[test]
    fn lint_ignores_brackets_inside_multiline_calc() {
        // The `]` inside the calc must not be read as the step's closer, and the
        // calc body must not be parsed as steps (no false "unknown step").
        let text = "Set Variable [$x = Let ( [\n    a = 1\n  ] ; a )]\nShow All Records";
        assert!(super::lint(text).is_empty());
    }

    // ── Opaque-by-default for unrecognized steps (#2) ──

    // A genuinely uncatalogued step (not in steps.toml) with arbitrary children.
    const UNKNOWN_STEP: &str = "<fmxmlsnippet type=\"FMObjectList\"><Step enable=\"True\" id=\"99999\" name=\"Totally Made Up Step\"><SomeWeirdChild foo=\"bar\"></SomeWeirdChild><Calculation><![CDATA[\"x\"]]></Calculation></Step></fmxmlsnippet>";

    #[test]
    fn unknown_step_roundtrips_xml_verbatim() {
        // decode → encode must reproduce the inner XML byte-for-byte.
        let script = xmss::parse_fmxml_snippet(UNKNOWN_STEP).unwrap();
        let rebuilt = xmss::build_xml_from_script(&script).unwrap();
        assert_eq!(rebuilt, UNKNOWN_STEP);
    }

    #[test]
    fn unknown_step_roundtrips_through_text() {
        // The full tool path: decode → format (.fmscript) → parse → encode.
        let script = xmss::parse_fmxml_snippet(UNKNOWN_STEP).unwrap();
        let text = super::format_script(&script);
        // The step id is carried inline because steps.toml can't supply it.
        assert!(text.contains("Totally Made Up Step #99999"));
        let script2 = super::parse_text_to_script(&text).unwrap();
        let rebuilt = xmss::build_xml_from_script(&script2).unwrap();
        assert_eq!(rebuilt, UNKNOWN_STEP);
    }

    #[test]
    fn lint_accepts_unknown_step_with_inline_id() {
        // With its inline #id it's a valid opaque step…
        assert!(super::lint("Totally Made Up Step #99999 [<Foo></Foo>]").is_empty());
        // …without one it's still flagged as unknown.
        assert!(!super::lint("Totally Made Up Step [<Foo></Foo>]").is_empty());
    }

    #[test]
    fn unknown_step_without_body_roundtrips() {
        let xml = "<fmxmlsnippet type=\"FMObjectList\"><Step enable=\"True\" id=\"42\" name=\"Mystery Step\"></Step></fmxmlsnippet>";
        let script = xmss::parse_fmxml_snippet(xml).unwrap();
        let text = super::format_script(&script);
        assert_eq!(text, "Mystery Step #42");
        let script2 = super::parse_text_to_script(&text).unwrap();
        assert_eq!(xmss::build_xml_from_script(&script2).unwrap(), xml);
    }

    #[test]
    fn nested_inline_block_comment_inside_open_step_does_not_eat_closing_bracket() {
        // Hand-authored FileMaker calcs can have a literal `/* ... */` comment
        // nested inside an otherwise-active, still-open multi-line step (e.g. to
        // disable a couple of extra JSONSetElement rows while keeping the call
        // itself live) — as opposed to the `/* StepName [...] */` shape this tool
        // generates for disabling a *whole* step. The two are told apart by what
        // follows `/* `: a step name (disable-whole-step) vs. `[` directly
        // (in-formula comment). The closing `]` may sit right after `*/` on the
        // same line; that `]` belongs to the enclosing step and must not be eaten.
        let text = "Set Variable [$$BF_Model = JSONSetElement ( $$BF_Model ; [\"a\" ; 1 ; 2] )\n\n/* [\"conf_mail\" ; $Conf_API_Graph ; 3 ];\n [\"plantillaMailQr\" ; $plantillaMailQr ; 1 ]*/]";
        let script = super::parse_text_to_script(text).unwrap();
        assert_eq!(script.steps.len(), 1);
        let calc = script.steps[0].calculation.as_deref().unwrap();
        assert!(calc.contains("JSONSetElement ( $$BF_Model ; [\"a\" ; 1 ; 2] )"));
        assert!(calc.contains("/* [\"conf_mail\""));
        assert!(calc.ends_with("*/"));
    }
}
