const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});
    const ghostty = b.dependency("ghostty", .{
        .target = target,
        .optimize = optimize,
    });

    // Reuse Ghostty's generated options and Unicode modules. A cached source
    // tree lets the bridge use Ghostty's private, safe C-handle accessor.
    const ghostty_c = ghostty.module("ghostty-vt-c");
    const sources = b.addWriteFiles();
    _ = sources.addCopyDirectory(ghostty.path("src"), "", .{});
    const root_source = sources.addCopyFile(b.path("src/search.zig"), "mightty_search.zig");

    const root = b.createModule(.{
        .root_source_file = root_source,
        .target = target,
        .optimize = optimize,
        .link_libc = true,
    });
    for ([_][]const u8{
        "build_options",
        "terminal_options",
        "unicode_tables",
        "symbols_tables",
        "uucode",
    }) |name| {
        root.addImport(name, ghostty_c.import_table.get(name) orelse
            @panic("Ghostty module configuration changed"));
    }

    const library = b.addLibrary(.{
        .name = "mightty-ghostty-search",
        .linkage = .static,
        .root_module = root,
    });
    library.use_llvm = true;
    library.root_module.pic = true;
    b.installArtifact(library);
}
