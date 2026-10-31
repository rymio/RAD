// Ready-made circuits and programs for Z80 Lab. "Easy Z80" follows Sergey Kiselev's Easy Z80
// single-board computer (github.com/skiselev/easy_z80, GPL-3.0): the same chips, reference
// designators and I/O ports (SIO at 80h–83h, CTC at 88h–8Bh, the GAL16V8 address decoder),
// redrawn for RAD from its documentation. It is simplified: 32K ROM + 32K RAM instead of the
// 512K chips with the 74HC670 paging registers, no MAX693 supervisor or RS-232 drivers, and an
// extra LED port at 40h (a 74HC273 + LED bar) so you can see a program run.

import type { Circuit, Placed, Wire } from './sim';

type Pin = [string, number];

class Board {
  parts: Placed[] = [];
  wires: Wire[] = [];
  private n = 0;
  add(ref: string, type: string, x: number, y: number, props: Record<string, any> = {}, rot = 0) {
    this.parts.push({ id: ref, type, ref, x, y, rot, props });
    return ref;
  }
  wire(a: Pin, b: Pin) {
    this.wires.push({ id: `w${++this.n}`, a: { part: a[0], pin: a[1] }, b: { part: b[0], pin: b[1] } });
  }
  /** The same signal to many pins (each wired from the first). */
  net(...pins: Pin[]) {
    for (let i = 1; i < pins.length; i++) this.wire(pins[i - 1], pins[i]);
  }
}

const Z80 = { A: [30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 1, 2, 3, 4, 5], D: [14, 15, 12, 8, 7, 9, 10, 13], CLK: 6, INT: 16, NMI: 17, MREQ: 19, IORQ: 20, RD: 21, WR: 22, WAIT: 24, BUSREQ: 25, RESET: 26, M1: 27, VCC: 11, GND: 29 };
const MEM = { A: [10, 9, 8, 7, 6, 5, 4, 3, 25, 24, 21, 23, 2, 26, 1], D: [11, 12, 13, 15, 16, 17, 18, 19], CE: 20, OE: 22, WE: 27, VCC: 28, GND: 14 };
const SIO = { D: [40, 1, 39, 2, 38, 3, 37, 4], INT: 5, M1: 8, VCC: 9, RXDA: 12, RXCA: 13, TXCA: 14, TXDA: 15, CLK: 20, RESET: 21, GND: 31, RD: 32, CD: 33, BA: 34, CE: 35, IORQ: 36 };
const CTC = { D: [25, 26, 27, 28, 1, 2, 3, 4], GND: 5, RD: 6, IORQ: 10, INT: 12, M1: 14, CLK: 15, CE: 16, RESET: 17, CS0: 18, CS1: 19, VCC: 24 };
const L273 = { MR: 1, Q: [2, 5, 6, 9, 12, 15, 16, 19], D: [3, 4, 7, 8, 13, 14, 17, 18], GND: 10, CP: 11, VCC: 20 };

export function easyZ80(): Circuit {
  const b = new Board();
  b.add('PS1', 'psu', 40, 40);
  b.add('QG1', 'osc', 40, 170, { hz: 10_000_000 });
  b.add('QG2', 'osc', 40, 330, { hz: 1_843_200 });
  b.add('U1', 'z80', 330, 60);
  b.add('U5', 'rom28c256', 700, 40);
  b.add('U4', 'ram62256', 700, 400);
  b.add('U9', 'gal16v8', 330, 560, {
    names: 'MREQ IORQ WR M1 A2 A3 A4 A5 A6 GND A7 RAMCS LEDWR A15 NC NC ROMCS SIOCS CTCCS VCC',
    equations: [
      '; Easy Z80 address decode (A15 in place of the paging register’s MA19)',
      '/ROMCS = /MREQ * /A15',
      '/RAMCS = /MREQ * A15',
      '/CTCCS = /IORQ * M1 * A7 * /A6 * /A5 * /A4 * A3 * /A2',
      '/SIOCS = /IORQ * M1 * A7 * /A6 * /A5 * /A4 * /A3 * /A2',
      '; added for RAD: the LED port at 40h–43h',
      '/LEDWR = /IORQ * M1 * /WR * /A7 * A6 * /A5 * /A4 * /A3 * /A2',
    ].join('\n'),
  });
  b.add('U3', 'sio', 1060, 40);
  b.add('U2', 'ctc', 1060, 560);
  b.add('J2', 'terminal', 1360, 120);
  b.add('U10', 'hc273', 700, 780);
  b.add('D2', 'ledbar', 940, 800);
  b.add('R2', 'resistor', 120, 520, { ohms: 10_000 }, 90);
  b.add('SW1', 'button', 120, 640, {}, 90);
  b.add('C24', 'capacitor', 40, 640, { farads: 1e-6 }, 90);
  b.add('RN1', 'rnet', 200, 500, { ohms: 4700 });
  b.add('RN4', 'rnet', 560, 560, { ohms: 4700 });
  const caps = ['U1', 'U5', 'U4', 'U9', 'U3', 'U2', 'U10'];
  caps.forEach((_, i) => b.add(`C${i + 1}`, 'capacitor', 40 + i * 70, 900, { farads: 1e-7 }, 90));

  const vcc: Pin = ['PS1', 1];
  const gnd: Pin = ['PS1', 2];
  // Power to every chip, and its decoupling capacitor.
  const power: [string, number, number][] = [
    ['U1', Z80.VCC, Z80.GND], ['U5', MEM.VCC, MEM.GND], ['U4', MEM.VCC, MEM.GND], ['U9', 20, 10], ['U3', SIO.VCC, SIO.GND], ['U2', CTC.VCC, CTC.GND], ['U10', L273.VCC, L273.GND],
    ['QG1', 8, 4], ['QG2', 8, 4],
  ];
  for (const [u, v, g] of power) {
    b.wire(vcc, [u, v]);
    b.wire(gnd, [u, g]);
  }
  caps.forEach((u, i) => {
    const [, v, g] = power.find((p) => p[0] === u)!;
    b.wire([u, v], [`C${i + 1}`, 1]);
    b.wire([u, g], [`C${i + 1}`, 2]);
  });
  b.wire(['D2', 9], gnd);
  b.wire(['J2', 3], gnd);

  // Clocks: 10 MHz to the CPU, SIO and CTC; 1.8432 MHz to the SIO's channel A (115200 baud at ×16).
  b.net(['QG1', 5], ['U1', Z80.CLK], ['U3', SIO.CLK], ['U2', CTC.CLK]);
  b.net(['QG2', 5], ['U3', SIO.RXCA], ['U3', SIO.TXCA]);

  // Reset: a 10k pull-up, the button and a capacitor to GND (the power-on reset).
  b.wire(vcc, ['R2', 1]);
  b.net(['R2', 2], ['U1', Z80.RESET], ['SW1', 1], ['C24', 1], ['U3', SIO.RESET], ['U2', CTC.RESET], ['U10', L273.MR]);
  b.wire(['SW1', 2], gnd);
  b.wire(['C24', 2], gnd);

  // Pull-ups on the CPU's inputs (RN1) and on the data bus (RN4, for interrupt mode 0).
  b.wire(vcc, ['RN1', 1]);
  b.wire(['RN1', 2], ['U1', Z80.INT]);
  b.wire(['RN1', 3], ['U1', Z80.NMI]);
  b.wire(['RN1', 4], ['U1', Z80.WAIT]);
  b.wire(['RN1', 5], ['U1', Z80.BUSREQ]);
  b.wire(vcc, ['RN4', 1]);

  // Address bus: A0–A14 to both memories; A0/A1 select the SIO and CTC registers.
  for (let i = 0; i < 15; i++) {
    b.wire(['U1', Z80.A[i]], ['U5', MEM.A[i]]);
    b.wire(['U5', MEM.A[i]], ['U4', MEM.A[i]]);
  }
  b.net(['U1', Z80.A[0]], ['U3', SIO.CD], ['U2', CTC.CS0]);
  b.net(['U1', Z80.A[1]], ['U3', SIO.BA], ['U2', CTC.CS1]);

  // Data bus: every chip's D0–D7 (and the pull-ups).
  for (let i = 0; i < 8; i++) {
    b.wire(['U1', Z80.D[i]], ['U5', MEM.D[i]]);
    b.wire(['U5', MEM.D[i]], ['U4', MEM.D[i]]);
    b.wire(['U1', Z80.D[i]], ['U3', SIO.D[i]]);
    b.wire(['U3', SIO.D[i]], ['U2', CTC.D[i]]);
    b.wire(['U4', MEM.D[i]], ['U10', L273.D[i]]);
    b.wire(['U1', Z80.D[i]], ['RN4', i + 2]);
    b.wire(['U10', L273.Q[i]], ['D2', i + 1]);
  }

  // Control bus.
  b.net(['U1', Z80.RD], ['U5', MEM.OE], ['U4', MEM.OE], ['U3', SIO.RD], ['U2', CTC.RD]);
  b.wire(['U1', Z80.WR], ['U4', MEM.WE]);
  b.wire(vcc, ['U5', MEM.WE]); // the ROM is never written
  b.net(['U1', Z80.IORQ], ['U3', SIO.IORQ], ['U2', CTC.IORQ]);
  b.net(['U1', Z80.M1], ['U3', SIO.M1], ['U2', CTC.M1]);
  b.net(['U1', Z80.INT], ['U3', SIO.INT], ['U2', CTC.INT]);

  // The GAL: CPU signals in, chip selects out.
  b.wire(['U1', Z80.MREQ], ['U9', 1]);
  b.wire(['U1', Z80.IORQ], ['U9', 2]);
  b.wire(['U1', Z80.WR], ['U9', 3]);
  b.wire(['U1', Z80.M1], ['U9', 4]);
  [2, 3, 4, 5, 6].forEach((a, i) => b.wire(['U1', Z80.A[a]], ['U9', 5 + i]));
  b.wire(['U1', Z80.A[7]], ['U9', 11]);
  b.wire(['U1', Z80.A[15]], ['U9', 14]);
  b.wire(['U9', 17], ['U5', MEM.CE]);
  b.wire(['U9', 12], ['U4', MEM.CE]);
  b.wire(['U9', 18], ['U3', SIO.CE]);
  b.wire(['U9', 19], ['U2', CTC.CE]);
  b.wire(['U9', 13], ['U10', L273.CP]);

  // The console: SIO channel A to the USB-serial adapter.
  b.wire(['U3', SIO.TXDA], ['J2', 1]);
  b.wire(['J2', 2], ['U3', SIO.RXDA]);
  return { parts: b.parts, wires: b.wires };
}

/** Just a power supply, to build your own. */
export function blank(): Circuit {
  return { parts: [{ id: 'PS1', type: 'psu', ref: 'PS1', x: 40, y: 40, props: {} }], wires: [] };
}

export const TEMPLATES = {
  easy: { label: 'Easy Z80 (wired, ready to run)', make: easyZ80 },
  blank: { label: 'Empty board (build it yourself)', make: blank },
};

export const PROGRAMS: Record<string, { label: string; source: string }> = {
  hello: {
    label: 'Hello (serial console, echoes what you type)',
    source: `; Hello: prints a greeting on the console (SIO channel A) and echoes what you type.
; Easy Z80 ports: SIO channel A data 80h, control 81h.
SIOA_D  equ 80h
SIOA_C  equ 81h

        org 0
start:  di
        ld sp,0             ; the stack grows down from FFFFh (RAM)
        ld hl,sio_init      ; set up channel A
        ld b,sio_len
        ld c,SIOA_C
        otir
        ld hl,hello
        call puts
echo:   call getc           ; wait for a key...
        call putc           ; ...and send it back
        jr echo

; Sends A to the console.
putc:   push af
.wait:  in a,(SIOA_C)       ; RR0
        bit 2,a             ; transmit buffer empty?
        jr z,.wait
        pop af
        out (SIOA_D),a
        ret

; Waits for a key; returns it in A.
getc:   in a,(SIOA_C)
        bit 0,a             ; a character received?
        jr z,getc
        in a,(SIOA_D)
        ret

; Sends the text at HL (ending in 0).
puts:   ld a,(hl)
        or a
        ret z
        call putc
        inc hl
        jr puts

sio_init:
        db 00011000b        ; WR0: channel reset
        db 4, 01000100b     ; WR4: clock x16, 1 stop bit, no parity
        db 3, 11000001b     ; WR3: receive 8 bits, receiver on
        db 5, 11101000b     ; WR5: DTR, send 8 bits, transmitter on
sio_len equ $-sio_init

hello:  db "Hello from your Z80!",13,10,"Type something: ",0
`,
  },
  blinky: {
    label: 'Blinky (counts on the LEDs at port 40h)',
    source: `; Blinky: counts up on the LED bar (the 74HC273 at port 40h).
; At a slow clock, make DELAY small (try 1) to see every count.
LEDS    equ 40h
DELAY   equ 20000

        org 0
start:  ld sp,0
        xor a
loop:   out (LEDS),a        ; the byte appears on the LEDs
        inc a
        ld bc,DELAY
wait:   dec bc
        ld d,a
        ld a,b
        or c                ; BC = 0 yet?
        ld a,d
        jr nz,wait
        jr loop
`,
  },
  timer: {
    label: 'Timer (CTC interrupts in mode 2)',
    source: `; Timer: the CTC interrupts the CPU about 150 times a second (interrupt mode 2);
; the interrupt routine counts and shows the count on the LEDs.
CTC0    equ 88h
LEDS    equ 40h
count   equ 8000h           ; a byte in RAM

        org 0
start:  di
        ld sp,0
        xor a
        ld (count),a
        ld a,vectors >> 8   ; I: the page of the vector table
        ld i,a
        im 2
        ld a,vectors & 0F8h ; the CTC's vector (channel 0 adds 0)
        out (CTC0),a
        ld a,10100111b      ; interrupts on, timer, prescaler 256, time constant next, reset
        out (CTC0),a
        xor a               ; time constant 256: 10 MHz / 256 / 256 = 153 Hz
        out (CTC0),a
        ei
idle:   halt                ; sleep until the next interrupt
        jr idle

tick:   push af
        ld a,(count)
        inc a
        ld (count),a
        out (LEDS),a
        pop af
        ei
        reti

        align 8
vectors: dw tick
`,
  },
};
