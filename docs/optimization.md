   The worst offenders, ranked by "a 90s game dev would rewrite this first":

  1. spinner.rs:133 — sync_files() allocates a new HashSet<String> every frame

  let current_paths: HashSet<String> = files.iter().map(|f| f.path.clone()).collect();
  Clones every path string, builds a HashSet, runs 60x/sec. Then at line 177:
  self.prev_paths = current_paths;
  self.prev_files = files.to_vec();
  Clones the entire file list every frame. A 90s dev would reuse the HashSet (clear + re-insert) and
  swap instead of clone.

  2. spinner.rs:442 — file.action.to_string() allocates a String per visible file, per frame

  Called 13 times per frame (VISIBLE_SLOTS*2+1). Display for an enum creates a new String. A 90s dev
  would use action.as_str() -> &'static str.

  3. spinner.rs:517 — format_age() allocates a String per frame via format!

  Called every frame for the showcase area. A 90s dev would write into a fixed buffer or use write!
  into a reusable String.

  4. state.rs:82-90 — hotfile.clone() pushed into both self.files AND delta

  Every event clones the HotFile twice (one for the vec, one for the delta). The path, filename, dir,
   full_dir, mime_type are all owned Strings — 5 heap allocations per clone. A 90s dev would use
  indices/IDs instead of cloned structs.

  5. state.rs:99-102 — Full sort + index rebuild on every event batch

  sort_by is O(n log n) on the entire file list after every batch. rebuild_index iterates and
  re-inserts every path into the HashMap. A 90s dev would insertion-sort (nearly sorted data = O(n))
  or maintain a sorted structure.

  6. torch.rs:182 — embers.remove(0) is O(n) shift on eviction

  When at capacity, remove(0) shifts all elements left. A 90s dev would use a ring buffer or
  swap-remove (like flames.rs already does correctly at line 276).

  7. trace_db.rs:304-312 — FieldCollector uses format! per field per span

  Every traced span/event builds strings with format!. On the render path with trace-level spans,
  this is called hundreds of times per frame. The MIN_LEVEL filter (DEBUG) saves us from the worst of
   it, but the format!("{:?}") calls still allocate for debug-level events.

  8. heat_glow.rs:185 — fire_column.resize() allocates on height change

  Minor — only happens on resize — but a 90s dev would allocate once at max size and track active
  length.

  9. CinderSystem uses Vec<Ember> with push/retain — variable allocation

  A 90s dev would use a fixed array [Ember; MAX_EMBERS] with an active count, like FlamePass already
  does. The cinder system is the one particle system that didn't follow the same pattern.

  What's already done right (90s style):
  - FlamePass — fixed [Particle; 512] array, swap-remove, no allocations in sim loop
  - heat_glow fire automaton — in-place mutation of a flat Vec<f32>, pure arithmetic
  - SpinnerState — physics are just float math, zero alloc
  - Rng — xorshift in flames.rs, no rand crate overhead
  The path a file takes through hotbar:

  events.jsonl → ClaudeCursor::read_new() → Vec<FileEvent>
      → HotState::apply_events() → Delta { added: Vec<HotFile> }
          → Db::insert_events()        (writes 5 Strings to SQLite)
          → rebuild_index()            (re-hashes every path)
          → sort()                     (re-sorts everything)
      → Arc<RwLock<HotState>>
          → panel reads .files()       (borrows slice)
              → spinner.sync_files()   (clones all paths into HashSet, clones all files)
              → draw_file_slot()       (action.to_string() per slot)
              → format_age()           (format! per frame)

  The same path string — say "/home/zack/dev/hotbar/main.rs" — gets heap-allocated at least 11 times
  on its journey through the system: once in the parser, once in FileEvent.path, once in
  file_event_to_hotfile, once in self.files.push, once in by_path.insert, once in delta.added.push,
  once in Db::insert_events bind param, once in sync_files HashSet, once in prev_files clone, once in
   arrivals HashMap key, and once more when the user pins/opens it.

  Here's what a 90s systems architect would restructure:

  1. Path intern table — eliminates ~80% of string cloning across the entire system

  One Vec<String> + HashMap<String, u32> owns every path. Everything else uses PathId(u32). The same
  4-byte ID flows through ingest → state → delta → panel → DB. A path is allocated exactly once, when
   first seen. HotFile shrinks from 5 owned Strings (path, filename, dir, full_dir, mime_type) to 1
  PathId + derived-on-demand fields. The filename, dir, mime_type are all computable from the path —
  store them in the intern table, compute once.

  2. Shared uniform buffer — 6 GPU uploads become 1

  Right now there are 6 queue.write_buffer calls per frame across chrome, heat_glow (uniform + fire),
   flames (uniform + particles), starburst. The four uniform uploads (chrome, heat_glow, flames,
  starburst) all contain overlapping data: resolution, time, heat_intensity. Pack them into a single
  FrameUniforms struct, one upload, one bind group. Each shader reads from the same buffer at
  different offsets. The particle and fire column uploads stay separate (they're variable-size data),
   but the 4 uniform uploads collapse to 1.

  3. Merge chrome + heat_glow into one render pass

  5 begin_render_pass calls per frame. Chrome uses LoadOp::Clear (it's the first pass, writes the
  base), heat_glow uses LoadOp::Load (additive on top). But they're both fullscreen triangles using
  the same vertex shader. Merge them into one render pass with two draw calls — clear once, draw
  chrome pipeline, draw heat_glow pipeline. Same for starburst (post-egui) — it's a single draw call
  that doesn't need its own pass, it could share the egui pass. That's 5 passes → 3 passes (pre-egui,
   egui, post-egui merged into egui).

  4. Double-buffer the file list instead of clone-per-frame

  sync_files() clones the entire &[HotFile] into prev_files every frame. Classic double-buffer: the
  state owns two vecs (files_a, files_b), swaps which is "current" vs "previous" each frame. The
  panel borrows both — zero allocation after the first frame. The HashSet for path diffing should be
  clear() + re-insert (reuses its allocation) instead of rebuilt from scratch.

  5. Static dispatch tables instead of per-frame string allocation

  Action::to_string() and Source::to_string() allocate a String via the Display trait. Add as_str()
  -> &'static str methods returning string literals. format_age() builds a new String every frame —
  use write! into a scratch buffer owned by the spinner state (one String reused across frames).

  6. Insertion-sort instead of full sort in apply_events

  Files arrive roughly in timestamp order. After inserting 1-10 new events into 200 files, a full
  sort_by is O(200 log 200). Binary-search for the insertion point + shift is O(log 200 + 10) per
  event. Or skip sorting entirely — maintain a BTreeMap<(i64, PathId), usize> that's always sorted,
  and derive the display-order slice from it.

  7. Unify cinder and flames particle patterns

  Flames uses [Particle; 512] fixed array + swap-remove (zero alloc, cache-friendly). Cinders uses
  Vec<Ember> + push/retain/remove(0) (allocates, O(n) shifts). Make cinders match flames: [Ember;
  MAX_EMBERS] + active count + swap-remove. Same data layout, same update loop structure.

  8. Write-behind DB batching

  Db::insert_events runs synchronously on the ingest path. Move DB writes to a background task:
  ingest → state (fast, in-memory) → mpsc channel → writer task batches and commits every 500ms. The
  hot path never touches SQLite. The panel reads state via the Arc, not the DB. DB is purely for
  persistence across restarts.

  9. Compile-gate trace-level spans in release builds

  The trace_span! calls in the render loop (torch_sprite, cinder_update, cinder_draw, spinner_draw,
  etc.) still evaluate their arguments even when the tracing subscriber filters them. Use
  #[cfg(debug_assertions)] or a cargo feature to compile them out entirely in release mode. The
  debug-level spans in the daemon are fine — they run on events, not per-frame.

  10. The fire automaton output can drive cinder spawning

  The heat_glow fire automaton produces a column of heat values [0.0..1.0]. The cinder system spawns
  independently based on file write events. If cinders sampled the fire automaton's column at the
  relevant Y position for their initial heat value, the two systems would be visually coherent
  (cinders are hottest where the glow is brightest) and you'd remove the independent heat calculation
   in the cinder spawner.

  ---
  Net effect estimate if all 10 were applied:

  ┌───────────────────────────────┬────────────────────────────┬─────────────────────────────────┐
  │            Metric             │           Before           │              After              │
  ├───────────────────────────────┼────────────────────────────┼─────────────────────────────────┤
  │ Heap allocs per frame         │ ~40-60 (strings, vecs,     │ ~2-4 (egui internals only)      │
  │                               │ hashsets)                  │                                 │
  ├───────────────────────────────┼────────────────────────────┼─────────────────────────────────┤
  │ GPU buffer uploads per frame  │ 6                          │ 3 (1 shared uniform + particles │
  │                               │                            │  + fire)                        │
  ├───────────────────────────────┼────────────────────────────┼─────────────────────────────────┤
  │ Render passes per frame       │ 5                          │ 3                               │
  ├───────────────────────────────┼────────────────────────────┼─────────────────────────────────┤
  │ Path string copies per ingest │ ~11 per file               │ 1 (first encounter only)        │
  │  cycle                        │                            │                                 │
  ├───────────────────────────────┼────────────────────────────┼─────────────────────────────────┤
  │ Bytes copied per sync_files   │ ~50KB (200 files * ~250B   │ 0 (pointer swap)                │
  │ call                          │ each)                      │                                 │
  ├───────────────────────────────┼────────────────────────────┼─────────────────────────────────┤
  │ apply_events sort cost        │ O(n log n) always          │ O(k log n) per batch            │
  └───────────────────────────────┴────────────────────────────┴─────────────────────────────────┘
