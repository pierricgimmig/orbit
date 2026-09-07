// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
'use strict';
let listeners = [], nativeAgent, callbacks, closeAgent, generationStorage, generation = 0;

// Mach-O offsets are relative to the selected architecture slice. Parse loaded
// segment commands, which also works for universal files without guessing which
// on-disk fat slice dyld selected.
function segments(module) {
    if (module.base.readU32() !== 0xfeedfacf) throw new Error('expected 64-bit Mach-O');
    const n = module.base.add(16).readU32();
    if (n > 4096) throw new Error('invalid Mach-O load command count');
    let command = module.base.add(32), result = [];
    for (let i = 0; i < n; i++) {
        const type = command.readU32(), size = command.add(4).readU32();
        if (size < 8 || size > 1048576) throw new Error('invalid Mach-O command');
        if (type === 0x19 && size >= 72) {
            result.push({ vm: command.add(24).readU64(),
                offset: command.add(40).readU64(), size: command.add(48).readU64(),
                executable: (command.add(60).readU32() & 4) !== 0 });
        }
        command = command.add(size);
    }
    const text = result.find(s => s.offset.compare(0) === 0 && s.size.compare(0) > 0);
    if (!text) throw new Error('Mach-O has no header segment');
    return result.map(s => ({ ...s, address: module.base.add(s.vm.sub(text.vm).toString()) }));
}

function addressOf(hook) {
    const module = Process.enumerateModules().find(m => m.path === hook.module_path);
    if (!module) throw new Error('module no longer loaded: ' + hook.module_path);
    const offset = uint64(hook.file_offset);
    if (Process.platform === 'darwin') {
        const seg = segments(module).find(s => s.executable && offset.compare(s.offset) >= 0 && offset.compare(s.offset.add(s.size)) < 0);
        if (seg) return seg.address.add(offset.sub(seg.offset).toString());
    } else {
        for (const r of Process.enumerateRanges('r-x')) {
            if (r.file && r.file.path === hook.module_path &&
                offset.compare(r.file.offset) >= 0 && offset.compare(uint64(r.file.offset).add(r.size)) < 0)
                return r.base.add(offset.sub(r.file.offset).toString());
        }
    }
    throw new Error('function offset is outside executable mappings: ' + hook.name);
}

rpc.exports = {
    symbols() {
        const result = [];
        for (const module of Process.enumerateModules()) {
            try {
                const segs = segments(module);
                for (const symbol of module.enumerateSymbols()) {
                    if (symbol.type !== 'function') continue;
                    const seg = segs.find(s => s.executable && symbol.address.compare(s.address) >= 0 &&
                        symbol.address.compare(s.address.add(s.size.toString())) < 0);
                    if (seg) result.push({name: symbol.name, module: module.name, module_path: module.path,
                        file_offset: seg.offset.add(uint64(symbol.address.sub(seg.address).toString())).toNumber(), size: symbol.size || 0});
                }
            } catch (_) { /* Some shared-cache images do not expose symbols. */ }
        }
        return result;
    },
    start(config) {
        if (generation !== 0) throw new Error('already recording');
        nativeAgent = Process.findModuleByName(config.agent.split('/').pop()) || Module.load(config.agent);
        const openAgent = new NativeFunction(nativeAgent.getExportByName('orbit_frida_open'), 'uint', ['pointer']);
        closeAgent = new NativeFunction(nativeAgent.getExportByName('orbit_frida_close'), 'void', ['uint']);
        generation = openAgent(Memory.allocUtf8String(config.transport));
        if (generation === 0) throw new Error('agent rejected transport (permissions, version, or another active collector)');
        generationStorage = Memory.alloc(4); generationStorage.writeU32(generation);
        try {
            callbacks = new CModule(`
#include <gum/guminterceptor.h>
extern const guint32 capture_generation;
extern guint64 orbit_now(void);
extern guint32 orbit_tid(void);
extern void orbit_close(guint32);
void finalize(void) { orbit_close(capture_generation); }
extern void orbit_record(guint32, guint64, guint64, guint64, guint32, guint32);
typedef struct { guint64 start, id; guint32 tid, depth; } Invocation;
void on_enter(GumInvocationContext * ctx) {
    Invocation * i = gum_invocation_context_get_listener_invocation_data(ctx, sizeof(Invocation));
    i->start = orbit_now();
    i->id = (guintptr) gum_invocation_context_get_listener_function_data(ctx);
    i->tid = orbit_tid();
    i->depth = gum_invocation_context_get_depth(ctx);
}
void on_leave(GumInvocationContext * ctx) {
    Invocation * i = gum_invocation_context_get_listener_invocation_data(ctx, sizeof(Invocation));
    orbit_record(capture_generation, i->id, i->start, orbit_now(), i->tid, i->depth);
}
`, { capture_generation: generationStorage, orbit_now: nativeAgent.getExportByName('orbit_frida_now'),
     orbit_record: nativeAgent.getExportByName('orbit_frida_record'),
     orbit_close: nativeAgent.getExportByName('orbit_frida_close'),
     orbit_tid: nativeAgent.getExportByName('orbit_frida_tid') });
            for (const hook of config.hooks)
                listeners.push(Interceptor.attach(addressOf(hook),
                    {onEnter: callbacks.on_enter, onLeave: callbacks.on_leave}, ptr(hook.function_id)));
            Interceptor.flush();
            return {armed: listeners.length, arch: Process.arch, platform: Process.platform};
        } catch (error) { this.stop(); throw error; }
    },
    stop() {
        for (const listener of listeners) listener.detach();
        listeners = [];
        Interceptor.flush();
        if (generation !== 0) closeAgent(generation);
        generation = 0;
        // Retain CModule while outstanding invocations may still reference its
        // callbacks. Frida owns safe script teardown after those calls return.
        return {stopped: true};
    }
};
