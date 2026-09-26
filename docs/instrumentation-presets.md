# Instrumentation presets

Presets save a hooked-function selection as a named JSON file. Open **More →
Instrumentation presets**, or **Presets** in the Functions tab or capture settings.

1. Select a process and hook the functions you want.
2. Give the preset a name, such as unreal-physics, and choose **Save selection**.
3. Later, select a process and choose **Load presets**. You can select several
   files together, or load more files later.

Loading adds to the current selection. For example, loading unreal-physics
and unreal-rendering selects their union; overlapping functions are hooked only
once. Existing selections stay selected. Presets affect the **next capture**,
like the Functions tab's Hooked column.

The files are suitable for version control and sharing. They contain module
**filenames** and exact function names (including overload signatures), never
installation directories, process IDs, addresses, or Orbit's runtime function IDs.
The Rust service resolves them against the selected process's complete symbol
index, independently of the Functions tab's result limit.

    {
      "version": 1,
      "name": "game-physics",
      "functions": [
        { "module": "GamePhysics.dll", "name": "Physics::World::Step(float)" },
        { "module": "GamePhysics.dll", "name": "Physics::SolveContacts()" }
      ]
    }

Use **Save selection** to get the actual module and function names for your build.
Moving an installation does not invalidate a preset. Renaming modules, changing
symbol names, or switching to a build without the same symbols can leave entries
unresolved. Library filenames may differ between operating systems. Presets do
not use fuzzy matching or silently substitute another overload. If several
addresses have the same module/name pair, Orbit reports it as ambiguous and
skips it. The results list matched, added, missing and ambiguous entries; expand
**Details** to inspect them or copy the full unresolved list.

Loaded presets remain in the current viewer session and are re-resolved when
you choose another process. **Apply again** restores any of their functions you
have since unhooked. **Forget loaded presets** stops that automatic reapplication
and keeps the current hook selection. The downloaded files are the persistent
copy: reload them after reopening the viewer, or send them to another user.

Version 1 presets work with the Rust service on Linux and macOS. They select
functions, not capture settings or an instrumentation backend.
