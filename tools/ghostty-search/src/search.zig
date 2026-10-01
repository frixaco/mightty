const std = @import("std");
const terminal = @import("terminal/main.zig");
const terminal_c = @import("terminal/c/terminal.zig");
const render_c = @import("terminal/c/render.zig");

extern fn ghostty_terminal_vt_write(terminal_c.Terminal, [*]const u8, usize) void;
extern fn ghostty_terminal_vt_write_until_ground(terminal_c.Terminal, [*]const u8, usize, *usize) c_int;

fn write(raw: terminal_c.Terminal, ptr: [*]const u8, len: usize) callconv(.c) u64 {
    const wrapper = raw orelse return 0;
    var input = ptr[0..len];
    var active = wrapper.terminal.modes.get(.synchronized_output);
    var begins: u64 = 0;
    while (input.len > 0) {
        var consumed: usize = 0;
        _ = ghostty_terminal_vt_write_until_ground(raw, input.ptr, input.len, &consumed);
        if (consumed > 0) {
            input = input[consumed..];
        } else {
            // Only divide at ESC; Ghostty owns all sequence/UTF-8 interpretation.
            const end = std.mem.indexOfScalar(u8, input, 0x1b) orelse input.len;
            const take = if (end == 0) 1 else end;
            ghostty_terminal_vt_write(raw, input.ptr, take);
            input = input[take..];
        }
        const next = wrapper.terminal.modes.get(.synchronized_output);
        if (next and !active) begins += 1;
        active = next;
    }
    return begins;
}

// The pinned RenderState update only changes these terminal dirty flags.
// Restore them even on allocation failure; no terminal borrow escapes this call.
fn observe(state: render_c.RenderState, raw: terminal_c.Terminal) callconv(.c) Result {
    const t = terminal_c.zigTerminal(raw) orelse return .invalid_value;
    if (state == null) return .invalid_value;
    const s = t.screens.active;
    const alloc = s.alloc;
    const row_dirty = alloc.alloc(bool, s.pages.rows) catch return .out_of_memory;
    defer alloc.free(row_dirty);
    const page_dirty = alloc.alloc(bool, s.pages.rows) catch return .out_of_memory;
    defer alloc.free(page_dirty);
    const top = s.pages.getTopLeft(.viewport);
    var iterator = top.pageIterator(.right_down, null);
    var y: usize = 0;
    var n: usize = 0;
    while (y < s.pages.rows) {
        const chunk = iterator.next() orelse return .invalid_value;
        const p = chunk.node.page();
        const take = @min(@as(usize, chunk.end - chunk.start), s.pages.rows - y);
        page_dirty[n] = p.dirty;
        n += 1;
        for (p.rows.ptr(p.memory)[chunk.start..][0..take], row_dirty[y..][0..take]) |row, *dirty| dirty.* = row.dirty;
        y += take;
    }
    const terminal_dirty = t.flags.dirty;
    const screen_dirty = s.dirty;
    defer {
        t.flags.dirty = terminal_dirty;
        s.dirty = screen_dirty;
        iterator = top.pageIterator(.right_down, null);
        y = 0;
        n = 0;
        while (y < s.pages.rows) {
            const chunk = iterator.next().?;
            const p = chunk.node.page();
            const take = @min(@as(usize, chunk.end - chunk.start), s.pages.rows - y);
            p.dirty = page_dirty[n];
            n += 1;
            for (p.rows.ptr(p.memory)[chunk.start..][0..take], row_dirty[y..][0..take]) |*row, dirty| row.dirty = dirty;
            y += take;
        }
    }
    t.flags.dirty.palette = true; // force a complete independent copy
    return @enumFromInt(@intFromEnum(render_c.update(state, raw)));
}

const Terminal = terminal.Terminal;
const Screen = terminal.Screen;
const ScreenKey = terminal.ScreenSet.Key;
const ScreenSearch = terminal.search.Screen;
const FlattenedHighlight = terminal.highlight.Flattened;

const Result = enum(c_int) {
    success = 0,
    out_of_memory = -1,
    invalid_value = -2,
    out_of_space = -3,
    no_value = -4,
};

const Step = enum(c_int) {
    pending = 0,
    complete = 1,
};

const Direction = enum(c_int) {
    next = 0,
    previous = 1,
};

const Range = extern struct {
    start_x: u16,
    start_y: u32,
    end_x: u16,
    end_y: u32,
};

const Search = struct {
    alloc: std.mem.Allocator,
    terminal: *Terminal,
    query: []u8,
    screen_key: ScreenKey,
    screen_generation: usize,
    screen: *Screen,
    state: ?ScreenSearch,

    fn init(terminal_ptr: *Terminal, query: []const u8) !*Search {
        const screen = terminal_ptr.screens.active;
        const alloc = screen.alloc;
        const owned_query = try alloc.dupe(u8, query);
        errdefer alloc.free(owned_query);

        const search = try alloc.create(Search);
        errdefer alloc.destroy(search);

        const screen_key = terminal_ptr.screens.active_key;
        const screen_generation = terminal_ptr.screens.generation(screen_key);
        search.* = .{
            .alloc = alloc,
            .terminal = terminal_ptr,
            .query = owned_query,
            .screen_key = screen_key,
            .screen_generation = screen_generation,
            .screen = screen,
            .state = try ScreenSearch.init(alloc, screen, owned_query),
        };
        return search;
    }

    fn deinit(self: *Search) void {
        self.deinitState();
        self.alloc.free(self.query);
        self.alloc.destroy(self);
    }

    fn deinitState(self: *Search) void {
        if (self.state) |*state| {
            if (self.screenIsValid()) {
                state.deinit();
            } else {
                state.deinitScreenInvalid();
            }
            self.state = null;
        }
    }

    fn screenIsValid(self: *const Search) bool {
        if (self.terminal.screens.generation(self.screen_key) != self.screen_generation) {
            return false;
        }
        return self.terminal.screens.get(self.screen_key) == self.screen;
    }

    fn refresh(self: *Search) !*ScreenSearch {
        const screen_key = self.terminal.screens.active_key;
        const screen_generation = self.terminal.screens.generation(screen_key);
        const screen = self.terminal.screens.active;
        if (screen_key != self.screen_key or
            screen_generation != self.screen_generation or
            screen != self.screen)
        {
            self.deinitState();
            self.screen_key = screen_key;
            self.screen_generation = screen_generation;
            self.screen = screen;
            self.state = try ScreenSearch.init(self.alloc, screen, self.query);
        }

        const state = if (self.state) |*state| state else {
            self.state = try ScreenSearch.init(self.alloc, screen, self.query);
            return &self.state.?;
        };
        try state.reloadActive();
        return state;
    }
};

fn new(
    terminal_raw: terminal_c.Terminal,
    query_ptr: ?[*]const u8,
    query_len: usize,
    out_search: ?*?*Search,
) callconv(.c) Result {
    const output = out_search orelse return .invalid_value;
    output.* = null;
    if (query_ptr == null and query_len != 0) return .invalid_value;

    const terminal_ptr = terminal_c.zigTerminal(terminal_raw) orelse
        return .invalid_value;
    const query = if (query_len == 0) "" else query_ptr.?[0..query_len];
    output.* = Search.init(terminal_ptr, query) catch return .out_of_memory;
    return .success;
}

fn free(search: ?*Search) callconv(.c) void {
    const value = search orelse return;
    value.deinit();
}

fn step(search: ?*Search, out_step: ?*Step) callconv(.c) Result {
    const value = search orelse return .invalid_value;
    const output = out_step orelse return .invalid_value;
    const state = value.refresh() catch return .out_of_memory;

    state.tick() catch |err| switch (err) {
        error.OutOfMemory => return .out_of_memory,
        error.FeedRequired => {
            state.feed() catch return .out_of_memory;
            output.* = .pending;
            return .success;
        },
        error.SearchComplete => {
            // Feed on a completed search removes results whose scrollback
            // pages were pruned after the last step.
            state.feed() catch return .out_of_memory;
            output.* = .complete;
            return .success;
        },
    };
    output.* = .pending;
    return .success;
}

fn ranges(
    search: ?*Search,
    output: ?[*]Range,
    capacity: usize,
    out_len: ?*usize,
) callconv(.c) Result {
    const value = search orelse return .invalid_value;
    const length = out_len orelse return .invalid_value;
    const state = value.refresh() catch return .out_of_memory;
    const matches = state.matches(value.alloc) catch return .out_of_memory;
    defer value.alloc.free(matches);

    length.* = matches.len;
    if (matches.len == 0) return .success;
    if (output == null or capacity < matches.len) return .out_of_space;
    for (matches, output.?[0..matches.len]) |highlight, *range| {
        range.* = highlightRange(value.screen, highlight) orelse
            return .invalid_value;
    }
    return .success;
}

fn select(
    search: ?*Search,
    direction: Direction,
    out_range: ?*Range,
) callconv(.c) Result {
    const value = search orelse return .invalid_value;
    const output = out_range orelse return .invalid_value;
    const state = value.refresh() catch return .out_of_memory;
    const moved = state.select(switch (direction) {
        .next => .next,
        .previous => .prev,
    }) catch return .out_of_memory;
    if (!moved) return .no_value;

    const selected = state.selectedMatch() orelse return .no_value;
    output.* = highlightRange(value.screen, selected) orelse
        return .invalid_value;
    return .success;
}

fn highlightRange(screen: *Screen, highlight: FlattenedHighlight) ?Range {
    const untracked = highlight.untracked();
    const start = screen.pages.pointFromPin(.screen, untracked.start) orelse return null;
    const end = screen.pages.pointFromPin(.screen, untracked.end) orelse return null;
    return .{
        .start_x = start.screen.x,
        .start_y = start.screen.y,
        .end_x = end.screen.x,
        .end_y = end.screen.y,
    };
}

fn probe(search: ?*Search, out_match: ?*bool, out_buffer_changed: ?*bool) callconv(.c) Result {
    const value = search orelse return .invalid_value;
    const matched = out_match orelse return .invalid_value;
    const changed = out_buffer_changed orelse return .invalid_value;
    changed.* = value.terminal.screens.active_key != value.screen_key or !value.screenIsValid();
    matched.* = false;
    if (changed.*) return .success;
    var progress: Step = .pending;
    const result = step(value, &progress);
    if (result != .success) return result;
    if (value.state) |*state| matched.* = state.matchesLen() > 0;
    return .success;
}

comptime {
    @export(&write, .{ .name = "mightty_ghostty_vt_write" });
    @export(&observe, .{ .name = "mightty_ghostty_render_observe" });
    @export(&new, .{ .name = "mightty_ghostty_search_new" });
    @export(&free, .{ .name = "mightty_ghostty_search_free" });
    @export(&probe, .{ .name = "mightty_ghostty_search_probe" });
    @export(&step, .{ .name = "mightty_ghostty_search_step" });
    @export(&ranges, .{ .name = "mightty_ghostty_search_ranges" });
    @export(&select, .{ .name = "mightty_ghostty_search_select" });
}
