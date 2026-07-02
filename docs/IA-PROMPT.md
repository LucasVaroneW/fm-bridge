# fm-bridge .fmscript Syntax Reference for AI

This document defines the exact syntax an AI must follow when generating
`.fmscript` files for `fm-bridge`. The format is a plain-text representation
of FileMaker scripts — one step per line, 2-space indentation for blocks.

**About FileMaker IDs — the rule the AI MUST follow:**

FileMaker links some step references by **numeric ID at paste time**, not by name.
When the ID is missing, FM accepts the paste but shows the reference as
`<unknown>` and the step is dead until a human opens FM and re-picks the target.

Two categories of references:

| Reference | Resolves by name? | Resolves by id? | Notes |
|---|---|---|---|
| `Perform Script` target | ❌ | ✅ | `#scriptId` REQUIRED |
| `Go to Layout` target | ❌ (shows `<unknown>`) | ✅ | `#layoutId` REQUIRED to link |
| `New Window` Layout target | ❌ | ✅ | `#layoutId` REQUIRED to link |
| `Set Field` target field | ✅ (FM resolves by `table::name`) | ✅ | id optional |
| `Perform Find` criterion field | ✅ (FM resolves by `table::name`) | ✅ | id optional |
| `Go to Object` object name | ✅ (it's a calc, evaluated at runtime) | n/a | no id concept |
| `Select Window` window name | ✅ (it's a calc) | n/a | no id concept |

**When the AI does NOT know the required id** (typically because the script is
being authored from scratch, not edited from a decoded copy), the AI MUST:

1. Emit the step **without the id** (the human will fix the link in FM).
2. Place a `# TODO:` comment immediately above the step naming the intended
   target so a human reviewer knows what to wire up. For example:
   ```
   # TODO: link Go to Layout to "Ta_SeccionesItemsPrecios" (id needed)
   Go to Layout ["Ta_SeccionesItemsPrecios"]
   ```

When the AI **does** know the id (e.g. when editing an `.fmscript` that came
from `fm-bridge read`), it MUST preserve the id verbatim.

---

## General Rules

* One step per line (multi-line cases noted below).
* Comments start with `# `. A line with just `#` is a comment with no text.
* A **truly empty line** (just `\n`) round-trips as a blank Script Workspace line.
* Disabled steps:
  * Single-line: prefix with `// ` (space after `//` is required).
  * Multi-line: wrap with `/* ... */` (the `/*` opens before the step name on
    the first line, `*/` is on its own line after the closing `]`). The block
    form prevents VSCode (and other editors) from treating an unclosed `"`
    inside the calc as a string that leaks through the rest of the file.
* Block indentation: 2 spaces per nesting level (`If`, `Loop`).
* Encoding: **UTF-8 without BOM**.

---

## Step Syntax by Type

### Set Variable

```
Set Variable [$varName = expression]
```

* `$varName` is required before `=`.
* `expression` is any valid FileMaker calculation.
* Multi-line calculations are allowed inside the brackets — preserve user
  indentation (tabs or spaces) of the calculation body literally.
* Repetition numbers like `$var[1]` are part of the variable name.

```
Set Variable [$counter = 0]
Set Variable [$result = ExecuteSQL (
    "SELECT COUNT(*) FROM Table
     WHERE flag = 0"
    ; "" ; ""
)]
Set Variable [$$globalVar = Get ( CurrentTimestamp )]
```

### If / Else If / Else / End If

```
If [condition]
  ...
Else If [condition]
  ...
Else
  ...
End If
```

Multi-line conditions inside `If` are formatted with **2 spaces of
continuation indent** per nesting level:

```
If [Get ( LayoutName ) = "Home"
  and
  not IsEmpty ( $user )]
  Show All Records
End If
```

The continuation indent is purely cosmetic — the parser strips it.

### Loop / End Loop / Exit Loop If

```
Loop
  Set Variable [$i = $i + 1]
  Exit Loop If [$i >= 10]
End Loop
```

### Set Error Capture / Allow User Abort

```
Set Error Capture [True]
Set Error Capture [False]
Allow User Abort [True]
Allow User Abort [False]
```

`Allow User Abort` always takes a state. Default in FM is `False`.

### Exit Script / Halt Script

```
Exit Script
Halt Script
```

### Show Custom Dialog

```
Show Custom Dialog [Title: "title"; Message: "message"; Buttons: "btn1", "btn2"]
```

* Sections separated by `;`. Buttons are `,`-separated inside `Buttons:`.
* Only the FIRST button has CommitState=True.
* String values use FileMaker calc syntax — literal strings need `"..."`.

```
Show Custom Dialog [Title: "Warning"; Message: "Values missing."; Buttons: "OK"]
Show Custom Dialog [Title: "Confirm"; Message: "Proceed?"; Buttons: "Yes", "No"]
```

### Set Field

```
Set Field [Table::Field; expression]
Set Field [Field; expression]
```

* `Table::` prefix is the FM Table Occurrence (TO) name. Strongly recommended
  to include it — FM resolves the field by `table+name` on paste.
* No numeric IDs. Do not emit `#915` or anything similar.

```
Set Field [Ofe_d_Sesiones::Ses_gOfeArtSel; $pk_Art]
Set Field [Contacts::Name; Get ( AccountName )]
Set Field [App::gTempBuffer; ""]
```

### Go to Record/Request/Page

```
Go to Record/Request/Page [First]
Go to Record/Request/Page [Last]
Go to Record/Request/Page [Next]
Go to Record/Request/Page [Previous]
Go to Record/Request/Page [Calc: $rowNumber]
```

Optional flags after `;`:
```
Go to Record/Request/Page [Next; Exit; NoInteract]
```

* `Exit` — exit after last record.
* `NoInteract` — suppress dialogs.

### Perform Script / Perform Script on Server

```
Perform Script ["ScriptName" #scriptId; parameter]
Perform Script ["ScriptName" #scriptId]
Perform Script on Server ["ScriptName" #scriptId; parameter]
```

* **`#scriptId` is REQUIRED.** Unlike Field/Layout names, FM does NOT reliably
  resolve scripts by name on paste — it needs the numeric ID to create the
  link. Discover the ID by copying that target script in FM and running
  `fm-bridge dump-ids`, OR keep the ID FM assigned when this same script was
  decoded from an existing FM file.
* `parameter` is optional, separated by `;`. It's a FM calc expression.

```
Perform Script ["Gen_Comprobar" #836; "1¶1"]
Perform Script ["Reload" #100]
Perform Script on Server ["BatchProcess" #245; $payload]
```

### Perform Find

Multi-line DSL — one section per `RequestRow`:

```
Perform Find [
  Find: Table::Field1 => value1; Table::Field2 => value2
  Omit: Table::Field3 => value3
]
```

* `Find:` opens an Include request. `Omit:` opens an Omit request.
* Criteria within a section are `;`-separated.
* Each criterion is `Table::Field => value`. The separator is **literally `=>`**
  (an arrow), with one space on each side.
* The `value` is the literal FM search expression. Examples:
  * `0` — exact value 0
  * `=` — empty (single `=` is FM's "find empty" operator)
  * `*` — non-empty
  * `>=10` — greater or equal to 10
  * `$pk_OfeVer` — value of a variable (resolved at find time)
  * `"hello"` — literal string `hello` (quotes optional unless value contains spaces or operators)
* Field references use Table::Name (no IDs).
* Single-section finds are still wrapped in the multi-line block for
  consistency — the parser also accepts everything on one line, but emit
  multi-line for readability.

```
Perform Find [
  Find: Contacts::Status => "Active"; Contacts::City => "Madrid"
  Omit: Contacts::DoNotEmail => 1
]

Perform Find [
  Find: Orders::Total => >=100
]
```

❌ DO NOT USE: raw `<Query>` XML inside the brackets. The parser expects the
DSL above.

### Go to Layout

```
Go to Layout ["LayoutName" #layoutId]
Go to Layout ["LayoutName"]
Go to Layout [original]
```

* `#layoutId` is **required** for FM to link the step on paste. Without it the
  step shows `Go to Layout [<unknown>]` in FM until a human re-picks the target.
* `original` → returns to the layout the user was on before the script started.

```
Go to Layout ["Ta_SeccionesItems" #2608]
Go to Layout ["Ta_SeccionesItemsPrecios" #2725]
Go to Layout [original]
```

When you don't know the id (authoring from scratch):
```
# TODO: link Go to Layout to "Ta_SeccionesItems" (id needed)
Go to Layout ["Ta_SeccionesItems"]
```

### Go to Object

```
Go to Object ["objectName"]
Go to Object ["objectName"; Rep: 2]
```

* Object name is a FM calc — `"literal"` strings need quotes.
* `Rep:` is optional and defaults to 1.

```
Go to Object ["gestionArticulos"]
Go to Object [$dynamicTarget]
Go to Object ["portalRow"; Rep: $rowNumber]
```

### Select Window

```
Select Window ["WindowName"]
Select Window [$variableHoldingName]
Select Window [Current]
Select Window [First]
Select Window [Last]
Select Window [Next]
Select Window [Previous]
```

### New Window

```
New Window [Style: Document; Layout: "LayoutName"; Height: 1; Width: 1; Top: -1000; Left: -1000]
```

* `Style:` one of `Document`, `Floating`, `Dialog`, `Card`. Default `Document`.
* `Layout:` target layout name. **Same rule as `Go to Layout`** — FM needs the
  layout id to link. Without an id the layout reference becomes `<unknown>`.
  (The `Layout:` field in `New Window` does not currently support `#id`
  inline; if you need to link a specific layout, copy a working `New Window`
  step from FM and edit only the geometry/style. For from-scratch use,
  include a `# TODO:` comment naming the intended layout.)
* `Height:`, `Width:`, `Top:`, `Left:` — FM calculation expressions (literal numbers, `$variables`, or expressions).
* All sections are optional; omit what you don't need.
* The other window styling attributes (Close/Minimize/Maximize/Resize/Toolbars/MenuBar) default to the standard FM Document chrome and are not exposed here.

```
New Window [Style: Document; Layout: "ContactList"]
New Window [Style: Floating; Layout: "Picker"; Width: 400; Height: 600]
New Window [Style: Document; Layout: "HiddenWorker"; Height: 1; Width: 1; Top: -1000; Left: -1000]
```

### Insert from URL

```
Insert from URL [Target: <target>; URL: <calc>; cURL: <calc>; Dialog: Off; VerifySSL; SelectAll; DontEncode]
```

* `Target:` is where the response goes. Two forms:
  * `$variableName` (or `$$globalVar`) — emits as `<Field>$var</Field>`.
  * `Table::FieldName` — emits as `<Field table="..." name="..."/>`. FM resolves
    by table+name on paste (same rule as `Set Field`).
* `URL:` is a FM calc expression. Quotes literal strings: `URL: "https://api.example.com/v1"`.
* `cURL:` is a FM calc expression with cURL options. Optional — omit when not needed.
  Common pattern: `cURL: "--request POST --header \"Content-Type: application/json\" --data @$payload"`.
* **Flags** (all default to off — emit only when needed, in any order):
  * `Dialog: Off` → suppresses FM's "With dialog" interactive prompt (FM stores
    this as `NoInteract=True`). Almost always set for headless API calls.
  * `VerifySSL` → enable SSL certificate verification. **FM's default is OFF**, which
    is insecure — emit this flag for production HTTPS calls.
  * `SelectAll` → select all of the target field's content before insertion.
  * `DontEncode` → skip FM's automatic URL encoding (use when the URL is already encoded).

Examples:

```
# Simple GET into a variable
Insert from URL [Target: $response; URL: "https://api.example.com/health"; Dialog: Off; VerifySSL]

# POST with JSON payload and bearer token, no dialog
Insert from URL [Target: $respuesta; URL: $endpointUrl; cURL: "--request POST --header \"Content-Type: application/json\" --header \"Authorization: Bearer " & $$token & "\" --data @$payload"; Dialog: Off; VerifySSL]

# Result into a real field
Insert from URL [Target: Logs::LastResponse; URL: $url; Dialog: Off]
```

Notes:
* The cURL expression uses FM-calc string concatenation with `&`. Variables
  like `$$token` are interpolated at runtime, not at script-paste time.
* Escape inner quotes as `\"` inside FM string literals.
* No numeric IDs needed anywhere — `Insert from URL` doesn't link to any
  FM object by id.

### Show All Records / Show Omitted Only / Show Custom Dialog / etc.

Simple parameterless or `plain` steps:

```
Show All Records
New Record/Request
Delete Record/Request
Delete All Records
Commit Records/Requests
Refresh Window
Close Window
```

### Adjust Window

```
Adjust Window [ResizeToFit]
Adjust Window [Maximize]
Adjust Window [Minimize]
Adjust Window [Restore]
Adjust Window [Hide]
```

### Perform JavaScript in Web Viewer

```
Perform JavaScript in Web Viewer [Object: "viewerName"; Function: "myFunc"; Param[0]: "arg1"; Param[1]: $arg2]
```

### Execute FileMaker Data API

```
Execute FileMaker Data API [$resultVar; jsonCalc]
```

---

## Comments

Comments are a step in FM (`# (comment)`). They can be standalone or interleaved.

```
# Single-line comment
#
# A second comment after a blank-comment separator
Set Variable [$x = 1]
```

A comment that spans multiple lines in FM Script Workspace (created by
pressing Enter inside a comment) is serialized with the `&#13;` entity to
keep it on one `.fmscript` line:

```
# Header line&#13;Body line 1&#13;Body line 2
```

Round-trip: `fm-bridge` emits `&#13;` between the lines on decode, and
re-encodes them as FM's internal CR on write.

---

## Empty Lines and Blank Comment Steps

A **truly empty line** in the `.fmscript` round-trips as a blank Script
Workspace line (the kind FM creates when you press Enter without typing
anything):

```
Set Variable [$a = 1]

Set Variable [$b = 2]
```

The empty line above is preserved as a blank step on paste.

A bare `#` (gato sin texto) is a comment step with empty text — visually
also a separator in FM but distinct from a truly empty line.

---

## Multi-line Calculations

For long calculations, the bracket content spans multiple lines. Open `[` is
on the step line; closing `]` is on the last line of the calc.

```
Set Variable [$query = Let (
    [
        ~base = "SELECT col FROM T WHERE flag = ?";
        ~bind = 0
    ] ;
    ExecuteSQL ( ~base ; "" ; "" ; ~bind )
)]
```

* The parser tracks bracket depth `[ ... ]`, parens `( ... )`, and string
  quotes `"..."` to detect the **outermost** closing `]`.
* Tabs and spaces inside the calc body are preserved verbatim.
* For `If [...]` only, the formatter adds 2 spaces of cosmetic continuation
  indent to each line. The parser strips them. You can author with or
  without the indent.

---

## Variable Naming

* Local variables: `$name`
* Global variables: `$$name`
* Repetition: `$name[1]`, `$name[N]` — the `[N]` is part of the variable name string.

---

## Steps with `opaque` Shape (Lossless XML Preservation)

A few steps carry an FM configuration too rich for a flat DSL. They are
**opaque**: the codec captures their inner XML verbatim so it always
round-trips byte-exact. On top of that, several render a **readable DSL** —
but only when that DSL rebuilds the original XML exactly. If it can't, the
raw XML is kept inside the brackets instead (never lossy).

Steps that render a readable DSL:

* `Import Records` — shows Source / Target / **Mapping**.
* `Commit Records/Requests` — shows its options.
* `Go to Related Record` — shows table, layout and options.

`Import Records` Mapping — each row is `[N] <field> #<id> <action>`, where
`[N]` is the **source column** that feeds that field (`[-]` = a target row
past the last source column). The `[N]` prefix is read-only annotation:
it's recomputed on render and ignored on write, so you can edit the field
list freely and it still round-trips.

```
Import Records [ ... 
  Mapping:
    [1] id #8 Import
    [2] nombre #67 Import
    [3]  #0 DoNotImport
]
```

Steps that stay as raw XML (no DSL yet — do not hand-edit):

* `Export Records`
* `Export Field Contents`

---

## Quick Reference Card

| Step | Format |
|---|---|
| Comment | `# text` or `#` |
| Blank line | (empty line) |
| Set Variable | `Set Variable [$name = expr]` |
| If / Else If | `If [cond]` / `Else If [cond]` |
| Else / End If | `Else` / `End If` |
| Loop / End Loop / Exit Loop If | `Loop` / `End Loop` / `Exit Loop If [cond]` |
| Set Error Capture | `Set Error Capture [True\|False]` |
| Allow User Abort | `Allow User Abort [True\|False]` |
| Exit / Halt Script | `Exit Script` / `Halt Script` |
| Show Custom Dialog | `Show Custom Dialog [Title: ..; Message: ..; Buttons: a, b]` |
| Set Field | `Set Field [Table::Field; expr]` |
| Go to Record | `Go to Record/Request/Page [First\|Last\|Next\|Previous]` |
| Go to Layout | `Go to Layout ["Name" #id]` or `[original]` (id REQUIRED to link) |
| Go to Object | `Go to Object ["objectName"]` |
| Select Window | `Select Window ["Name"]` or `[Current\|First\|...]` |
| New Window | `New Window [Style: ..; Layout: "..."; Height: H; Width: W; Top: T; Left: L]` |
| Adjust Window | `Adjust Window [ResizeToFit\|Maximize\|...]` |
| Perform Script | `Perform Script ["Name" #id; param]` |
| Perform Script on Server | `Perform Script on Server ["Name" #id; param]` |
| Perform Find | multi-line `Find:` / `Omit:` DSL with `T::F => value` |
| Perform JS in WebViewer | `[Object: ..; Function: ..; Param[0]: ..]` |
| Execute FM Data API | `[$target; jsonCalc]` |
| Insert from URL | `[Target: $var; URL: calc; cURL: calc; Dialog: Off; VerifySSL]` |
| Plain steps | `Commit Records/Requests`, `New Record/Request`, `Delete Record/Request`, `Show All Records`, `Refresh Window`, `Close Window`, etc. |

---

## Common Mistakes to Avoid

* ❌ Writing raw `<Query>...</Query>` XML inside `Perform Find`. Use the
  `Find:` / `Omit:` DSL.
* ❌ Writing raw `<Layout id="2725"/>` XML in `Go to Layout`. Use the
  `["Name" #2725]` text form.
* ❌ Omitting the `#scriptId` in `Perform Script`. FM needs it to link.
* ❌ Omitting `#layoutId` on `Go to Layout` without also adding a `# TODO:`
  comment above. A bare `Go to Layout ["Name"]` pastes as `<unknown>` and a
  reviewer has no way to know what layout was intended.
* ❌ UTF-16 or any encoding other than UTF-8 without BOM. On Windows,
  pipe-redirecting `fm-bridge read > file.fmscript` from PowerShell corrupts
  the output — pass the path as an argument instead:
  `fm-bridge read file.fmscript`.
* ❌ Mixing tabs and 2-space indent at the block level. Use 2 spaces for
  block indent. Tabs inside calc bodies are fine (and preserved).
* ❌ Using `=` instead of `=>` as the field/value separator in `Perform Find`.
  The literal arrow `=>` is required.

---

## End-to-End Example

```
# Marca como favorito y reordena
Set Error Capture [True]
Allow User Abort [False]
Set Variable [$pk = Get ( ScriptParameter )]
If [IsEmpty ( $pk )]
  Show Custom Dialog [Title: "Error"; Message: "Missing PK"; Buttons: "OK"]
  Halt Script
End If
Go to Layout ["Contacts"]
Perform Find [
  Find: Contacts::PK => $pk
]
If [Get ( FoundCount ) = 0]
  Show Custom Dialog [Title: "Not found"; Message: "No matching contact"; Buttons: "OK"]
  Halt Script
End If
Set Field [Contacts::Favorite; 1]
Set Field [Contacts::FavoriteOrder; Get ( FoundCount ) + 1]
Commit Records/Requests
Go to Layout [original]
```
