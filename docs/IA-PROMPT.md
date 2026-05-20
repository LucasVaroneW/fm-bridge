# fm-bridge .fmscript Syntax Reference for AI

This document defines the exact syntax an AI must follow when generating
`.fmscript` files for `fm-bridge`. The format is a plain-text representation
of FileMaker scripts — one step per line, 2-space indentation for blocks.

---

## General Rules

* One step per line.
* Comments start with `#`. A line with just `#` (no text) is a valid blank
  separator comment.
* Disabled steps are prefixed with `// ` (space after `//` is required).
* Block indentation: 2 spaces per nesting level.
* Empty lines are ignored.

---

## Step Syntax by Type

### Set Variable

```
Set Variable [$varName = expression]
```

Examples:
```
Set Variable [$counter = 0]
Set Variable [$message = "Starting migration..."]
Set Variable [$result = ExecuteSQL ( "SELECT COUNT(*) FROM Table" ; "" ; "" )]
Set Variable [$$globalVar = Get ( CurrentTimestamp )]
```

* `$varName` is required before `=`.
* `expression` is any valid FileMaker calculation.
* Multiline calculations: indent continuation lines by 2 extra spaces inside
  the brackets.
* Repetition numbers like `$var[1]` are part of the variable name.

❌ DO NOT USE: `Set Variable [ $var; Value:expr ]` — this is internal XMSS format.

### If / Else If / Else / End If

```
If [ condition ]
  ...
Else If [ condition ]
  ...
Else
  ...
End If
```

### Loop / End Loop / Exit Loop If

```
Loop
  ...
  Exit Loop If [ condition ]
End Loop
```

### Set Error Capture

```
Set Error Capture [True]
Set Error Capture [False]
```

### Allow User Abort

```
Allow User Abort
```

### Exit Script

```
Exit Script
```

### Halt Script

```
Halt Script
```

### Show Custom Dialog

```
Show Custom Dialog [Title: "title"; Message: "message"; Buttons: "btn1", "btn2"]
```

* All three parts (Title, Message, Buttons) are separated by semicolons.
* `Buttons:` is optional (defaults to OK button).
* Only the FIRST button has CommitState=True (the "OK" equivalent).

Examples:
```
Show Custom Dialog [Title: "Warning"; Message: "Values are missing."]
Show Custom Dialog [Title: "Confirm"; Message: "Proceed?"; Buttons: "Yes", "No"]
```

❌ DO NOT USE: `Show Custom Dialog [ "title" ; "message" ]` — missing field keys.

### Set Field

```
Set Field [Table::Field; expression]
```

Examples:
```
Set Field [App::gTempBuffer; $data]
Set Field [Contacts::Name; Get ( AccountName )]
```

### Go to Record/Request/Page

```
Go to Record/Request/Page [First]
Go to Record/Request/Page [Last]
Go to Record/Request/Page [Next]
Go to Record/Request/Page [Previous]
```

### Perform Script

```
Perform Script ["ScriptName" #id; parameter]
Perform Script ["ScriptName" #id]
Perform Script [Get ( ScriptName )]
Perform Script on Server ["ScriptName" #id; parameter]
```

* `#id` is the FileMaker script ID number (required for copy-paste to link
  correctly in FM).
* `parameter` is optional and separated by `;`.

### Perform Find

```
Perform Find [criteria]
```

### Commit Records/Requests

```
Commit Records/Requests
```

### Refresh Window

```
Refresh Window
```

### Delete Record/Request

```
Delete Record/Request
```

---

## Comments

```
# This is a comment with text
#
# Sección de configuración
# ------------------------
Set Variable [$x = 1]
```

Lines starting with `#` are comments. A bare `#` is a separator line.

---

## Multiline Calculations

For long calculations, the bracket content spans multiple lines:

```
Set Variable [$query =
    ExecuteSQL (
        "SELECT col1, col2
         FROM Table
         WHERE flag = 0"
        ; "" ; ""
    )
]
If [ not IsEmpty ( $field )
and (
    IsEmpty ( $other )
    or
    $value < 0
)]
    ...
```

The parser tracks bracket depth `[ ... ]` and string quotes to correctly
detect the closing `]`.

---

## Variable Naming

* Local variables: `$name`
* Global variables: `$$name`
* Repetition: `$name[1]`, `$name[N]` (the `[N]` is part of the variable name string)

---

## Steps NOT yet fully supported (plain shape — content ignored on encode)

These steps have shape `plain` in `steps.toml` — their bracket content is
stored as raw text but may not serialize correctly back to FM:

* Import Records
* Export Field Contents

When generating scripts with these steps, use FileMaker's native UI to
configure the import/export mapping after pasting.

---

## File Encoding

`.fmscript` files MUST be UTF-8 without BOM. `fm-bridge write` supports
fallback to UTF-16 LE and Windows-1252, but UTF-8 is the canonical format.

---

## Quick Reference Card

| Step | Format |
|---|---|
| Comment | `# text` or `#` |
| Set Variable | `Set Variable [$name = expr]` |
| If | `If [ cond ]` |
| Else If | `Else If [ cond ]` |
| Else | `Else` |
| End If | `End If` |
| Loop | `Loop` |
| End Loop | `End Loop` |
| Exit Loop If | `Exit Loop If [ cond ]` |
| Set Error Capture | `Set Error Capture [True|False]` |
| Allow User Abort | `Allow User Abort` |
| Exit Script | `Exit Script` |
| Halt Script | `Halt Script` |
| Show Custom Dialog | `Show Custom Dialog [Title: ...; Message: ...; Buttons: ...]` |
| Set Field | `Set Field [Table::Field; expr]` |
| Go to Record | `Go to Record/Request/Page [First|Last|Next|Previous]` |
| Perform Script | `Perform Script ["Name" #id; param]` |
| Perform Script on Server | `Perform Script on Server ["Name" #id; param]` |
| Perform Find | `Perform Find [criteria]` |
| Commit Records | `Commit Records/Requests` |
| Refresh Window | `Refresh Window` |
| Delete Record | `Delete Record/Request` |
