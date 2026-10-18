// RAD's C64: a small C layer over floooh's chips C64 emulation (chips/, zlib licence) that
// crates/rad-c64/src/ffi.rs calls. It adds what RAD needs on top of c64.h:
//
//   - running a number of clock ticks with an optional per-tick callback (the lab's tracing)
//     and "traps": stop when the CPU is about to run one of a few addresses (the KERNAL's
//     LOAD, so RAD can serve files from a D64 disk image itself);
//   - the keyboard as a physical 8×8 matrix (and RESTORE), set directly;
//   - the cartridge port (8K/16K ROMs and a few bank-switching schemes);
//   - save states without c64.h's shared static buffer (several C64s run at once in RAD).
#include <stdint.h>
#include <stdbool.h>
#include <stdlib.h>
#include <string.h>

// Never abort RAD over an emulator assertion.
#define NDEBUG
#include <assert.h>
#define CHIPS_ASSERT(c) ((void)0)

#define RAD_CART_MAX (1 << 20)
#define RAD_CART_BANKS (RAD_CART_MAX / 0x4000)

// The cartridge (kept inside c64_t so save states carry it).
#define RAD_C64_EXTRA                                                                     \
    struct {                                                                              \
        bool present;                                                                     \
        uint16_t type;          /* CRT hardware type */                                   \
        bool exrom, game;       /* lines asserted (low) */                                \
        bool exrom0, game0;     /* at power on */                                         \
        uint16_t bank;                                                                    \
        uint8_t has_h[RAD_CART_BANKS]; /* a bank has its own ROMH half */                 \
        uint8_t rom[RAD_CART_MAX];     /* bank n: ROML at n*16K, ROMH at n*16K + 8K */    \
    } cart;

// (c64_t is an anonymous struct: the hooks take it as void*.)
static void rad_cart_map(void* sys);
static uint64_t rad_cart_io(void* sys, uint64_t pins);
#define RAD_C64_MAP(sys) rad_cart_map(sys)
#define RAD_C64_IO12(sys, pins) rad_cart_io(sys, pins)

#define CHIPS_IMPL
#include "chips/chips_common.h"
#include "chips/m6502.h"
#include "chips/m6526.h"
#include "chips/m6569.h"
#include "chips/m6581.h"
#include "chips/kbd.h"
#include "chips/mem.h"
#include "chips/clk.h"
#include "chips/c1530.h"
#include "chips/m6522.h"
#include "chips/c1541.h"
#include "chips/c64.h"

static void rad_cart_map(void* p) {
    c64_t* sys = (c64_t*)p;
    // 8000..9FFF: ROML when EXROM is low and LORAM+HIRAM are set; A000..BFFF: ROMH when
    // GAME is low too (16K mode) and HIRAM is set. Otherwise the C64's own map.
    const bool lo = sys->cpu_port & C64_CPUPORT_LORAM, hi = sys->cpu_port & C64_CPUPORT_HIRAM;
    bool roml = false, romh = false;
    if (sys->cart.present && sys->cart.exrom) {
        roml = lo && hi;
        romh = sys->cart.game && hi;
    }
    const uint16_t b = sys->cart.bank % RAD_CART_BANKS;
    uint8_t* bank = sys->cart.rom + b * 0x4000;
    if (roml) {
        mem_map_rw(&sys->mem_cpu, 0, 0x8000, 0x2000, bank, sys->ram + 0x8000);
    } else {
        mem_map_ram(&sys->mem_cpu, 0, 0x8000, 0x2000, sys->ram + 0x8000);
    }
    if (romh) {
        mem_map_rw(&sys->mem_cpu, 0, 0xA000, 0x2000, sys->cart.has_h[b] ? bank + 0x2000 : bank, sys->ram + 0xA000);
    }
}

static uint64_t rad_cart_io(void* p, uint64_t pins) {
    c64_t* sys = (c64_t*)p;
    if (!sys->cart.present) {
        return pins;
    }
    const uint16_t addr = M6502_GET_ADDR(pins);
    const bool rd = pins & M6502_RW;
    const bool io1 = addr < 0xDF00;
    const uint8_t v = M6502_GET_DATA(pins);
    bool changed = false;
    switch (sys->cart.type) {
        case 5: // Ocean: write the bank to DE00
            if (!rd && io1) { sys->cart.bank = v & 0x3F; changed = true; }
            break;
        case 15: // C64 Game System: write to DE00+n selects bank n, reading resets to bank 0
            if (io1) { sys->cart.bank = rd ? 0 : (addr & 0x3F); changed = true; }
            break;
        case 17: // Dinamic: read DE00+n to select bank n
            if (rd && io1) { sys->cart.bank = addr & 0x0F; changed = true; }
            break;
        case 19: // Magic Desk / Domark / HES Australia: bank in bits 0..6, bit 7 switches it off
            if (!rd && io1) { sys->cart.bank = v & 0x7F; sys->cart.exrom = !(v & 0x80); changed = true; }
            break;
        default:
            break;
    }
    if (changed) {
        _c64_update_memory_map(sys);
    }
    return pins;
}

// ------------------------------------------------------------------ the machine

typedef void (*rc64_tick_fn)(void* user, uint64_t pins);

typedef struct {
    c64_t sys;
    float audio[16384];
    int audio_len;
    uint64_t prev_pins;
    uint16_t traps[8];
    int ntraps;
    int trapped;    // the trap address hit by the last rc64_run, or -1
    uint16_t op_pc; // where the instruction running now started
} rc64_t;

static void rc64_audio_cb(const float* samples, int num, void* user) {
    rc64_t* r = (rc64_t*)user;
    for (int i = 0; i < num && r->audio_len < (int)(sizeof(r->audio) / sizeof(float)); i++) {
        r->audio[r->audio_len++] = samples[i];
    }
}

// The physical keyboard: key k (0..63) is row k/8 (CIA-1 port A bit) and column k%8 (port B
// bit); 64 is RESTORE. chips' kbd_t is used only for its scan-out masks.
static void rc64_apply_matrix(c64_t* sys, const uint8_t* keys, int n) {
    kbd_t* kbd = &sys->kbd;
    memset(kbd->scanout_column_masks, 0, sizeof(kbd->scanout_column_masks));
    memset(kbd->scanout_line_masks, 0, sizeof(kbd->scanout_line_masks));
    for (int i = 0; i < n; i++) {
        const int k = keys[i];
        if (k < 64) {
            const int line = k >> 3, col = k & 7;
            kbd->scanout_column_masks[line] |= (uint16_t)(1 << col);
            kbd->scanout_line_masks[col] |= (uint16_t)(1 << line);
        } else if (k == 64) {
            kbd->scanout_column_masks[8] |= 1;
            kbd->scanout_line_masks[0] |= (1 << 8);
        }
    }
    kbd->cur_column_mask = 0xFFFF;
    kbd->cur_line_mask = 0xFFFF;
    kbd->cur_scanout_line_mask = 0;
    kbd->cur_scanout_column_mask = 0;
}

rc64_t* rc64_new(const uint8_t* chars, const uint8_t* basic, const uint8_t* kernal, int sample_rate) {
    rc64_t* r = (rc64_t*)calloc(1, sizeof(rc64_t));
    if (!r) {
        return 0;
    }
    c64_init(&r->sys, &(c64_desc_t){
        .c1530_enabled = true,
        .joystick_type = C64_JOYSTICKTYPE_DIGITAL_12,
        .audio = { .callback = { .func = rc64_audio_cb, .user_data = r }, .num_samples = 64, .sample_rate = sample_rate, .volume = 0.6f },
        .roms = {
            .chars = { .ptr = (void*)chars, .size = 0x1000 },
            .basic = { .ptr = (void*)basic, .size = 0x2000 },
            .kernal = { .ptr = (void*)kernal, .size = 0x2000 },
        },
    });
    rc64_apply_matrix(&r->sys, 0, 0);
    r->trapped = -1;
    return r;
}

void rc64_free(rc64_t* r) {
    if (r) {
        free(r->sys.c1530.buf);
        c64_discard(&r->sys);
        free(r);
    }
}

void rc64_reset(rc64_t* r) {
    r->sys.cart.bank = 0;
    r->sys.cart.exrom = r->sys.cart.exrom0;
    r->sys.cart.game = r->sys.cart.game0;
    c64_reset(&r->sys);
    rc64_apply_matrix(&r->sys, 0, 0);
    r->audio_len = 0;
}

void rc64_set_sample_rate(rc64_t* r, int hz) {
    m6581_init(&r->sys.sid, &(m6581_desc_t){ .tick_hz = C64_FREQUENCY, .sound_hz = hz, .magnitude = 0.6f });
    r->audio_len = 0;
}

void rc64_set_traps(rc64_t* r, const uint16_t* addrs, int n) {
    r->ntraps = n > 8 ? 8 : n;
    for (int i = 0; i < r->ntraps; i++) {
        r->traps[i] = addrs[i];
    }
}

int rc64_trapped(rc64_t* r) {
    return r->trapped;
}

// Runs up to `ticks` clock cycles; stops early at a trap (the CPU about to run a trap address:
// rc64_trapped says which). `fn` (optional) sees every tick's pins. Returns the ticks run.
uint32_t rc64_run(rc64_t* r, uint32_t ticks, rc64_tick_fn fn, void* user) {
    c64_t* sys = &r->sys;
    uint64_t pins = sys->pins;
    uint64_t prev = r->prev_pins;
    uint32_t i = 0;
    r->trapped = -1;
    while (i < ticks) {
        pins = _c64_tick(sys, pins);
        i++;
        if (fn) {
            fn(user, pins);
        }
        // A new instruction (the CPU stalled by the VIC repeats the same fetch).
        if ((pins & M6502_SYNC) && !(prev & M6502_SYNC)) {
            const uint16_t a = M6502_GET_ADDR(pins);
            r->op_pc = a;
            for (int t = 0; t < r->ntraps; t++) {
                if (r->traps[t] == a) {
                    r->trapped = a;
                }
            }
            if (r->trapped >= 0) {
                prev = pins;
                break;
            }
        }
        prev = pins;
    }
    sys->pins = pins;
    r->prev_pins = prev;
    return i;
}

// Continue at `pc` instead of the instruction about to run (only right after a trap).
void rc64_jump(rc64_t* r, uint16_t pc) {
    r->op_pc = pc;
    c64_t* sys = &r->sys;
    uint64_t pins = sys->pins & ~0xFFFFULL;
    pins |= pc | M6502_SYNC | M6502_RW;
    M6502_SET_DATA(pins, mem_rd(&sys->mem_cpu, pc));
    sys->cpu.PC = pc;
    sys->pins = pins;
}

typedef struct {
    uint16_t pc;
    uint8_t a, x, y, s, p;
} rc64_regs_t;

void rc64_get_regs(rc64_t* r, rc64_regs_t* out) {
    m6502_t* c = &r->sys.cpu;
    // At an instruction fetch the PC register is the fetch address.
    out->pc = (r->sys.pins & M6502_SYNC) ? M6502_GET_ADDR(r->sys.pins) : r->op_pc;
    out->a = c->A;
    out->x = c->X;
    out->y = c->Y;
    out->s = c->S;
    out->p = c->P;
}

void rc64_set_regs(rc64_t* r, const rc64_regs_t* in) {
    m6502_t* c = &r->sys.cpu;
    c->A = in->a;
    c->X = in->x;
    c->Y = in->y;
    c->S = in->s;
    c->P = in->p;
}

// Memory as the CPU sees it now. In the I/O area: colour RAM, the VIC-II's registers, and
// open bus (0xFF) for the rest (reading the CIAs here would change them).
uint8_t rc64_peek(rc64_t* r, uint16_t addr) {
    c64_t* sys = &r->sys;
    if (sys->io_mapped && (addr & 0xF000) == 0xD000) {
        if (addr < 0xD400) {
            const uint8_t reg = addr & 0x3F;
            return reg < M6569_NUM_REGS ? sys->vic.reg.regs[reg] : 0xFF;
        }
        if (addr >= 0xD800 && addr < 0xDC00) {
            return sys->color_ram[addr & 0x3FF] | 0xF0;
        }
        if (addr >= 0xDD00 && addr < 0xDE00 && (addr & 0x0F) == 0) {
            return sys->cia_2.pa.pins;
        }
        return 0xFF;
    }
    if (addr < 2) {
        return addr == 0 ? sys->cpu.io_ddr : sys->cpu_port;
    }
    return mem_rd(&sys->mem_cpu, addr);
}

// Writes a byte as the CPU would to RAM (under ROM, it lands in the RAM below); colour RAM in
// the I/O area; other I/O is left alone.
void rc64_poke(rc64_t* r, uint16_t addr, uint8_t v) {
    c64_t* sys = &r->sys;
    if (sys->io_mapped && (addr & 0xF000) == 0xD000) {
        if (addr >= 0xD800 && addr < 0xDC00) {
            sys->color_ram[addr & 0x3FF] = v & 0x0F;
        }
        return;
    }
    if (addr < 2) {
        return;
    }
    mem_wr(&sys->mem_cpu, addr, v);
}

uint8_t* rc64_ram(rc64_t* r) { return r->sys.ram; }
const uint8_t* rc64_vic_regs(rc64_t* r) { return r->sys.vic.reg.regs; }
uint16_t rc64_vic_bank(rc64_t* r) { return r->sys.vic_bank_select; }
uint8_t rc64_cpu_port(rc64_t* r) { return r->sys.cpu_port; }
// For tracing: the CPU port's current value, readable while the machine runs.
const uint8_t* rc64_cpu_port_ptr(rc64_t* r) { return &r->sys.cpu_port; }
const uint8_t* rc64_rom_kernal(rc64_t* r) { return r->sys.rom_kernal; }

const uint8_t* rc64_framebuffer(rc64_t* r, int* width, int* height, int* stride) {
    chips_rect_t s = m6569_screen(&r->sys.vic);
    *width = s.width;
    *height = s.height;
    *stride = M6569_FRAMEBUFFER_WIDTH;
    return r->sys.fb;
}

int rc64_take_audio(rc64_t* r, float* out, int max) {
    int n = r->audio_len < max ? r->audio_len : max;
    memcpy(out, r->audio, n * sizeof(float));
    r->audio_len = 0;
    return n;
}

void rc64_keys(rc64_t* r, const uint8_t* keys, int n) {
    rc64_apply_matrix(&r->sys, keys, n);
}

void rc64_joystick(rc64_t* r, uint8_t port1, uint8_t port2) {
    c64_joystick(&r->sys, port1, port2);
}

bool rc64_quickload(rc64_t* r, const uint8_t* data, size_t len) {
    return c64_quickload(&r->sys, (chips_range_t){ .ptr = (void*)data, .size = len });
}

// ------------------------------------------------------------------ tape

bool rc64_tape_insert(rc64_t* r, const uint8_t* data, size_t len) {
    c1530_t* t = &r->sys.c1530;
    if (len > t->cap) {
        uint8_t* b = (uint8_t*)realloc(t->buf, len);
        if (!b) {
            return false;
        }
        t->buf = b;
        t->cap = (uint32_t)len;
    }
    return c64_insert_tape(&r->sys, (chips_range_t){ .ptr = (void*)data, .size = len });
}
void rc64_tape_remove(rc64_t* r) { c64_remove_tape(&r->sys); }
void rc64_tape_play(rc64_t* r) { c64_tape_play(&r->sys); }
void rc64_tape_stop(rc64_t* r) { c64_tape_stop(&r->sys); }
bool rc64_tape_motor(rc64_t* r) { return c64_is_tape_motor_on(&r->sys); }
// Play pressed (the sense line low).
bool rc64_tape_pressed(rc64_t* r) { return !(r->sys.cas_port & C64_CASPORT_SENSE); }
void rc64_tape_pos(rc64_t* r, uint32_t* pos, uint32_t* size) {
    *pos = r->sys.c1530.pos;
    *size = r->sys.c1530.size;
}
// Moves the tape to a byte position in its pulses (for save states).
void rc64_tape_seek(rc64_t* r, uint32_t pos) {
    r->sys.c1530.pos = pos < r->sys.c1530.size ? pos : r->sys.c1530.size;
    r->sys.c1530.pulse_count = 0;
}

void rc64_tape_rewind(rc64_t* r) {
    r->sys.c1530.pos = 0;
    r->sys.c1530.pulse_count = 0;
}

// ------------------------------------------------------------------ cartridge

// `rom` holds banks of 16K (ROML then ROMH), `has_h` one byte per bank. Takes effect at reset.
bool rc64_cart_insert(rc64_t* r, uint16_t type, bool exrom, bool game, const uint8_t* rom, size_t len, const uint8_t* has_h) {
    if (len > RAD_CART_MAX) {
        return false;
    }
    c64_t* sys = &r->sys;
    memset(sys->cart.rom, 0xFF, RAD_CART_MAX);
    memcpy(sys->cart.rom, rom, len);
    memcpy(sys->cart.has_h, has_h, RAD_CART_BANKS);
    sys->cart.present = true;
    sys->cart.type = type;
    sys->cart.exrom0 = exrom;
    sys->cart.game0 = game;
    return true;
}

void rc64_cart_remove(rc64_t* r) {
    r->sys.cart.present = false;
    r->sys.cart.exrom0 = r->sys.cart.game0 = false;
}

bool rc64_cart_present(rc64_t* r) { return r->sys.cart.present; }

// ------------------------------------------------------------------ save states

size_t rc64_state_size(void) { return sizeof(c64_t); }
uint32_t rc64_state_version(void) { return C64_SNAPSHOT_VERSION; }

void rc64_save(rc64_t* r, uint8_t* out) {
    c64_t* snap = (c64_t*)out;
    c64_save_snapshot(&r->sys, snap);
    // Not part of the machine's state (and pointers would be meaningless later).
    snap->audio.callback.func = 0;
    snap->audio.callback.user_data = 0;
    snap->c1530.buf = 0;
    snap->c1530.cap = 0;
}

// c64_load_snapshot without its static buffer (two machines may load at once).
bool rc64_load(rc64_t* r, const uint8_t* in) {
    c64_t* im = (c64_t*)malloc(sizeof(c64_t));
    if (!im) {
        return false;
    }
    c64_t* sys = &r->sys;
    memcpy(im, in, sizeof(c64_t));
    chips_debug_snapshot_onload(&im->debug, &sys->debug);
    chips_audio_callback_snapshot_onload(&im->audio.callback, &sys->audio.callback);
    m6502_snapshot_onload(&im->cpu, &sys->cpu);
    m6569_snapshot_onload(&im->vic, &sys->vic);
    mem_snapshot_onload(&im->mem_cpu, sys);
    mem_snapshot_onload(&im->mem_vic, sys);
    c1530_snapshot_onload(&im->c1530, &sys->c1530);
    // The tape's pulses stay where they are (the caller re-inserts the same tape first).
    im->c1530.buf = sys->c1530.buf;
    im->c1530.cap = sys->c1530.cap;
    if (im->c1530.size > im->c1530.cap) {
        im->c1530.size = 0;
        im->c1530.pos = 0;
    }
    c1541_snapshot_onload(&im->c1541, &sys->c1541, sys);
    memcpy(sys, im, sizeof(c64_t));
    free(im);
    r->audio_len = 0;
    r->prev_pins = 0;
    return true;
}

