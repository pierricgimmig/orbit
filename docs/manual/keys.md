# Keys and mouse

Taken from the viewer’s key handler and the track gestures. A text field keeps the keys (the scope box, the tracks box, a report filter, the process filter), except **Escape**, which clears the box that has focus. **R**, **I**, **F2** and **F3** also do nothing while the Self pane is the one taking keys.

## Keys

| Key | Action |
|---|---|
| **X** | Start or stop a capture. Hidden on a static file (an opened trace or capture) |
| **R** | Toggle the report. More → “Report   R” |
| **I** | Open Inspector |
| **F2** | Toggle the Self pane. More → “Self   F2” |
| **F3** | Toggle Benchmark. More → “Benchmark   F3” |
| **Space** | Toggle Follow latest |
| **Home** | Fit the capture to its content. The window is at least 1 µs wide |
| **Ctrl+O** / **⌘O** | Open a file |
| **Ctrl+Shift+P** / **⌘⇧P** | Open the process list. Up, Down and Enter move and select inside it |
| **Escape** | Clear the scope search, a Live highlight, the selection and a rectangle drag |
| **A** / **D** | Pan in time. Turns Follow off |
| **Left** / **Right** | Pan in time when nothing is selected. With a selection, nudge it |
| **W** / **S** | Zoom around the cursor. Turns Follow off |
| **Up** / **Down** | Scroll tracks, when nothing is selected |
| **Page Up** / **Page Down** | Scroll tracks by a page |
| **Ctrl+C** / **⌘C** | Copy the report table on Flat, Top-down, Bottom-up, Modules and Functions: the selected rows, or the whole table. A focused text field keeps its own copy. Code uses the **Copy** pill |

More also lists “Pan time: A / D”, “Zoom: W / S” and “Scroll tracks: ↑ / ↓, Page Up / Down”.

## Mouse

| Gesture | Where | Action |
|---|---|---|
| Wheel | Ruler | Zoom time |
| Wheel | Tracks | Scroll tracks |
| Ctrl+wheel, ⌘+wheel, or pinch | Tracks | Zoom around the cursor |
| Left-drag | Anywhere but a sample bar | Pan |
| Left-drag | A thread’s sample bar | Select that thread’s samples |
| Right-drag | Anywhere, including the ruler | Select every thread’s samples in the range |
| Shift+drag | Sample bar or right-drag | Add to the sample selection |
| Ctrl+drag or ⌘+drag | Tracks | Marquee-select scopes and copy the report. Does not zoom |
| Double-click | A scope | Zoom to 1.1× its duration |
| Double-click | Ruler | Fit, same as Home |
| Click | Thread header | Focus that thread, or release it |
| Click | Sample tick | Copy its call stack |
| Click | Empty canvas | Clear the selection |
| Right-click | A scope | **Sampling report for this scope**, **Highlight every instance** |
| Right-click | A function, in a report or on a flame bar | Hook or unhook it. The report item is **Hook function for dynamic instrumentation**. The flame item is **Hook function**. **Show disassembly and source** opens Code |

## Page URL

| Query | Effect |
|---|---|
| `?report=` | Open a tab: `flat`, `top_down`, `bottom_up`, `modules`, `live`, `flame`, `functions`, `code`, `inspector`, `selection` |
| `?tracks=` | Fill the tracks filter. `+` and `%20` are spaces |
| `?collapse=scheduler` | Start with the scheduler track folded |
| `?theme=` | `orbit`, `dracula`, `nord`, `gruvbox`, `solarized`, `solarized-light`, `gruvbox-light` |
| `?webgpu` | Try WebGPU instead of WebGL2 |
| `?capture=` | Open a `.orbit.stream` with no service. The site’s embedded viewer uses this |
