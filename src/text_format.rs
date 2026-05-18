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

    // Comments are special: "# text"
    if step.name == steps::COMMENT_NAME {
        let text = step.text.as_deref().unwrap_or("");
        return format!("{}# {}", indent, text);
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
            // Set Field: shows "[Table::Name; calc]" or just "[calc]" if no target.
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
        Some(StepShape::Comment) | Some(StepShape::Plain) | None => {
            // Fallback: show any calc or text we have
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
    }

    line
}

/// Parse plain text into a structured script.
/// Handles multiline bracket content by collecting lines until `]` is found.
pub fn parse_text_to_script(text: &str) -> Result<FmScript, String> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut steps = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;

    let resolve_id = |name: &str| -> Result<u32, String> {
        steps::id_for_en(name).ok_or_else(|| format!(
            "Step '{}' has no FileMaker ID in steps.toml. \
             Copy this step in FileMaker and run `fm-bridge dump-ids` to discover its id, \
             then add `id = N` to its entry in steps.toml.",
            name
        ))
    };

    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() { i += 1; continue; }

        let leading_spaces = line.len() - line.trim_start().len();
        let indent = (leading_spaces / 2) as u32;

        let enabled = !line.trim().starts_with("// ");
        let content = if line.trim().starts_with("// ") { &line.trim()[3..] } else { line.trim() };

        // Comment lines
        if content.starts_with("# ") {
            steps.push(ScriptStep {
                name: steps::COMMENT_NAME.to_string(),
                enable: true,
                id: resolve_id(steps::COMMENT_NAME)?,
                text: Some(content[2..].to_string()),
                calculation: None, var_name: None, repetition: None,
                object_name: None, function_name: None, parameters: Vec::new(),
                restore_state: None, set_state: None,
                dialog_title: None, dialog_message: None, dialog_buttons: Vec::new(),
                field_result: None, field_target: None, field_table: None, field_numeric_id: None,
                script_target_name: None, script_target_id: None, current_script_mode: None,
                goto_location: None, goto_exit_after_last: None, goto_no_interact: None,
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
            let step_name = &content[..idx];
            let first_chunk = &content[idx + 2..];

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
                    return Err(format!("Unclosed `[` in step '{}'", step_name));
                }
                bracket_content.push('\n');
                current_text = lines[i];
            }
            i += 1;

            let step = build_step_from_name(step_name, Some(&bracket_content), enabled, resolve_id(step_name)?, indent);
            steps.push(step);
        } else {
            // No bracket content
            let step = build_step_from_name(content, None, enabled, resolve_id(content)?, indent);
            steps.push(step);
            i += 1;
        }
    }

    Ok(FmScript { steps })
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
                indent_level: indent,
            }
        }
        Some(StepShape::FieldAndCalc) => {
            let (table, target, calc) = parse_field_and_calc_content(content);
            ScriptStep {
                name: name.to_string(), enable: enabled, id,
                text: None, calculation: calc,
                var_name: None, repetition: None,
                object_name: None, function_name: None, parameters: Vec::new(),
                restore_state: None, set_state: None,
                dialog_title: None, dialog_message: None, dialog_buttons: Vec::new(),
                field_result: None, field_target: target, field_table: table, field_numeric_id: None,
                script_target_name: None, script_target_id: None, current_script_mode: None,
                goto_location: None, goto_exit_after_last: None, goto_no_interact: None,
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
                indent_level: indent,
            }
        }
        // Calculation, Plain, or unknown — store content as calculation
        Some(StepShape::Calculation) | Some(StepShape::Plain) | None => ScriptStep {
            name: name.to_string(), enable: enabled, id,
            text: None, calculation: content.map(|c| c.to_string()),
            var_name: None, repetition: None,
            object_name: None, function_name: None, parameters: Vec::new(),
            restore_state: None, set_state: None,
            dialog_title: None, dialog_message: None, dialog_buttons: Vec::new(),
            field_result: None, field_target: None, field_table: None, field_numeric_id: None,
                script_target_name: None, script_target_id: None, current_script_mode: None,
                goto_location: None, goto_exit_after_last: None, goto_no_interact: None,
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

/// Parse Set Field content: `Table::Field; calc` or `Field; calc` or `calc` or `Table::Field;`.
/// Split on the first `;` at bracket depth 0 (so semicolons inside calcs don't trigger).
fn parse_field_and_calc_content(content: Option<&str>) -> (Option<String>, Option<String>, Option<String>) {
    let content = match content {
        Some(c) => c,
        None => return (None, None, None),
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
        None => return (None, None, Some(content.trim().to_string())),
    };

    let (table, name) = if let Some(idx) = target_str.find("::") {
        (Some(target_str[..idx].to_string()), Some(target_str[idx + 2..].to_string()))
    } else if target_str.is_empty() {
        (None, None)
    } else {
        (None, Some(target_str))
    };

    let calc = if calc_str.is_empty() { None } else { Some(calc_str) };
    (table, name, calc)
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
