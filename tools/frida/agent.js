// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
'use strict';
let listeners = [], nativeAgent, callbacks, closeAgent, generationStorage, hookStorage = [], generation = 0;

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

// Resolve one API instance before loading our fallback agent. Static SDKs may
// expose symbols without exporting them. Initialized SDKs also publish an API
// descriptor, which the native guard prefers and which needs no symbol lookup.
function targetApi(agentName) {
    for (const m of Process.enumerateModules()) {
        if (m.name === agentName) continue;
        const found = {};
        for (const name of ['orbit_init', 'orbit_start_dynamic', 'orbit_stop', 'orbit_start']) {
            const address = m.findExportByName(name);
            if (address) found[name] = address;
        }
        if (!found.orbit_start_dynamic || !found.orbit_init || !found.orbit_stop) {
            try {
                for (const symbol of m.enumerateSymbols()) {
                    const name = symbol.name.replace(/^_/, '');
                    if (['orbit_init', 'orbit_start_dynamic', 'orbit_stop', 'orbit_start'].includes(name))
                        found[name] = symbol.address;
                }
            } catch (_) {}
        }
        if (found.orbit_init && found.orbit_start_dynamic && found.orbit_stop)
            return [found.orbit_init, found.orbit_start_dynamic, found.orbit_stop];
    }
    return [ptr(0), ptr(0), ptr(0)];
}

rpc.exports = {
    symbols() {
        const result = [];
        for (const module of Process.enumerateModules()) {
            try {
                const segs = segments(module);
                for (const symbol of module.enumerateSymbols()) {
                    // Mach-O code symbols are 'section', not ELF's 'function'.
                    const machCode = symbol.type === 'section' && symbol.section &&
                        symbol.section.id.endsWith('.__text');
                    if (symbol.type !== 'function' && !machCode) continue;
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
        const agentName = config.agent.split('/').pop();
        const api = targetApi(agentName);
        nativeAgent = Process.findModuleByName(agentName) || Module.load(config.agent);
        const openAgent = new NativeFunction(nativeAgent.getExportByName('orbit_frida_open'), 'uint', ['pointer', 'pointer', 'pointer', 'pointer']);
        closeAgent = new NativeFunction(nativeAgent.getExportByName('orbit_frida_close'), 'void', ['uint']);
        generation = openAgent(Memory.allocUtf8String(config.transport), ...api);
        if (generation === 0) throw new Error('agent rejected transport (permissions, hidden/incompatible Orbit API, or another active collector)');
        generationStorage = Memory.alloc(4); generationStorage.writeU32(generation);
        try {
            callbacks = new CModule(`
#include <gum/guminterceptor.h>
extern const guint32 capture_generation;
extern guint64 orbit_start(guint32, const char *, gsize);
extern void orbit_stop(guint32, guint64);
extern void orbit_close(guint32);
void finalize(void) { orbit_close(capture_generation); }
typedef struct { const char * name; gsize len; } Hook;
void on_enter(GumInvocationContext * ctx) {
    guint64 * handle = gum_invocation_context_get_listener_invocation_data(ctx, sizeof(guint64));
    const Hook * hook = gum_invocation_context_get_listener_function_data(ctx);
    *handle = orbit_start(capture_generation, hook->name, hook->len);
}
void on_leave(GumInvocationContext * ctx) {
    guint64 * handle = gum_invocation_context_get_listener_invocation_data(ctx, sizeof(guint64));
    orbit_stop(capture_generation, *handle);
}
`, { capture_generation: generationStorage,
     orbit_start: nativeAgent.getExportByName('orbit_frida_start'),
     orbit_stop: nativeAgent.getExportByName('orbit_frida_stop'),
     orbit_close: nativeAgent.getExportByName('orbit_frida_close') });
            for (const hook of config.hooks) {
                const name = Memory.allocUtf8String(hook.name);
                const data = Memory.alloc(Process.pointerSize * 2);
                data.writePointer(name);
                // strlen is evaluated once at setup, never on the callback path.
                let length = 0; while (name.add(length).readU8() !== 0) length++;
                data.add(Process.pointerSize).writeU64(length);
                hookStorage.push({name, data});
                listeners.push(Interceptor.attach(addressOf(hook),
                    {onEnter: callbacks.on_enter, onLeave: callbacks.on_leave}, data));
            }
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
