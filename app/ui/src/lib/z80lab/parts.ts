// Z80 Lab parts: what can be put on the canvas, with every pin (number, name, direction) as on
// the real chip's datasheet. The Z80 family pinouts were checked against the Easy Z80 board's
// netlist (github.com/skiselev/easy_z80); the 74HC and memory pinouts are the JEDEC/TI ones.
//
// A pin's `bus` is the computer signal it is normally wired to (a ROM's /OE goes to the CPU's
// /RD), which "Connect matching pins" uses.

export type PinKind = 'in' | 'out' | 'io' | 'oc' | 'pwr' | 'gnd' | 'pas';
export interface PinDef {
  n: number;
  /** As on the datasheet; a leading "/" means active low (drawn with a bar over it). */
  name: string;
  kind: PinKind;
  bus?: string;
  desc?: string;
}
export type Category = 'Processor' | 'Memory' | 'Peripherals' | 'Logic' | 'Clock & reset' | 'Passive' | 'Output' | 'Power';
export interface PartDef {
  type: string;
  label: string;
  /** Reference designator prefix (U1, R3…). */
  ref: string;
  category: Category;
  /** dip: a chip in a DIP package of `pins` pins (pins missing from the list are not connected).
   *  two: a two-terminal part drawn as a symbol. box: a module with pins down its right side. */
  pkg: { kind: 'dip'; pins: number } | { kind: 'two' } | { kind: 'box'; w: number; h: number };
  pins: PinDef[];
  desc: string;
  doc?: string;
  props?: Record<string, string | number | boolean>;
}

const ZILOG = 'https://www.zilog.com/docs/z80';
const TI = (p: string) => `https://www.ti.com/lit/ds/symlink/${p}.pdf`;

const A = (i: number, kind: PinKind = 'in'): Omit<PinDef, 'n'> => ({ name: `A${i}`, kind, bus: `A${i}` });
const D = (i: number): Omit<PinDef, 'n'> => ({ name: `D${i}`, kind: 'io', bus: `D${i}` });
const VCC: Omit<PinDef, 'n'> = { name: 'VCC', kind: 'pwr', bus: 'VCC', desc: '+5 V supply' };
const GND: Omit<PinDef, 'n'> = { name: 'GND', kind: 'gnd', bus: 'GND', desc: 'Ground (0 V)' };
const pins = (list: Record<number, Omit<PinDef, 'n'>>): PinDef[] => Object.entries(list).map(([n, p]) => ({ n: Number(n), ...p }));

/** A 74HC gate chip with four 2-input gates (00, 08, 32: the same pins). */
function quad(type: string, label: string, gate: string, desc: string, doc: string): PartDef {
  const g = (i: number, base: number, out: number): Record<number, Omit<PinDef, 'n'>> => ({
    [base]: { name: `${i}A`, kind: 'in' },
    [base + 1]: { name: `${i}B`, kind: 'in' },
    [out]: { name: `${i}Y`, kind: 'out', desc: `${gate} of ${i}A and ${i}B` },
  });
  return {
    type,
    label,
    ref: 'U',
    category: 'Logic',
    pkg: { kind: 'dip', pins: 14 },
    pins: pins({ ...g(1, 1, 3), ...g(2, 4, 6), 7: GND, 8: { name: '3Y', kind: 'out', desc: `${gate} of 3A and 3B` }, 9: { name: '3A', kind: 'in' }, 10: { name: '3B', kind: 'in' }, 11: { name: '4Y', kind: 'out', desc: `${gate} of 4A and 4B` }, 12: { name: '4A', kind: 'in' }, 13: { name: '4B', kind: 'in' }, 14: VCC }),
    desc,
    doc,
  };
}

/** 32K memories in the JEDEC 28-pin package (28C256 EEPROM, 62256 SRAM). */
function mem32k(type: string, label: string, desc: string, doc: string): PartDef {
  return {
    type,
    label,
    ref: 'U',
    category: 'Memory',
    pkg: { kind: 'dip', pins: 28 },
    pins: pins({
      1: A(14), 2: A(12), 3: A(7), 4: A(6), 5: A(5), 6: A(4), 7: A(3), 8: A(2), 9: A(1), 10: A(0),
      11: D(0), 12: D(1), 13: D(2), 14: GND, 15: D(3), 16: D(4), 17: D(5), 18: D(6), 19: D(7),
      20: { name: '/CE', kind: 'in', desc: 'Chip enable: low selects this chip (from the address decoder)' },
      21: A(10),
      22: { name: '/OE', kind: 'in', bus: '/RD', desc: 'Output enable: low puts the byte on D0–D7 (the CPU’s /RD)' },
      23: A(11), 24: A(9), 25: A(8), 26: A(13),
      27: { name: '/WE', kind: 'in', bus: '/WR', desc: 'Write enable: low stores D0–D7 (the CPU’s /WR; tie high on a ROM)' },
      28: VCC,
    }),
    desc,
    doc,
  };
}

export const PARTS: PartDef[] = [
  {
    type: 'z80',
    label: 'Z80 CPU (Z84C00)',
    ref: 'U',
    category: 'Processor',
    pkg: { kind: 'dip', pins: 40 },
    pins: pins({
      1: A(11, 'out'), 2: A(12, 'out'), 3: A(13, 'out'), 4: A(14, 'out'), 5: A(15, 'out'),
      6: { name: 'CLK', kind: 'in', bus: 'CLK', desc: 'Clock input: every T-state is one clock cycle' },
      7: D(4), 8: D(3), 9: D(5), 10: D(6), 11: VCC, 12: D(2), 13: D(7), 14: D(0), 15: D(1),
      16: { name: '/INT', kind: 'in', bus: '/INT', desc: 'Maskable interrupt request (needs a pull-up)' },
      17: { name: '/NMI', kind: 'in', bus: '/NMI', desc: 'Non-maskable interrupt (needs a pull-up)' },
      18: { name: '/HALT', kind: 'out', bus: '/HALT', desc: 'Low while the CPU is halted' },
      19: { name: '/MREQ', kind: 'out', bus: '/MREQ', desc: 'Memory request: the address is a memory address' },
      20: { name: '/IORQ', kind: 'out', bus: '/IORQ', desc: 'I/O request: the low byte of the address is a port' },
      21: { name: '/RD', kind: 'out', bus: '/RD', desc: 'Read: the CPU wants a byte on D0–D7' },
      22: { name: '/WR', kind: 'out', bus: '/WR', desc: 'Write: the CPU has put a byte on D0–D7' },
      23: { name: '/BUSACK', kind: 'out', bus: '/BUSACK', desc: 'Bus acknowledge (DMA)' },
      24: { name: '/WAIT', kind: 'in', bus: '/WAIT', desc: 'Low makes the CPU wait (needs a pull-up)' },
      25: { name: '/BUSREQ', kind: 'in', bus: '/BUSREQ', desc: 'Bus request for DMA (needs a pull-up)' },
      26: { name: '/RESET', kind: 'in', bus: '/RESET', desc: 'Low resets the CPU; it starts at 0000h when it goes high' },
      27: { name: '/M1', kind: 'out', bus: '/M1', desc: 'Machine cycle one: low while fetching an opcode' },
      28: { name: '/RFSH', kind: 'out', bus: '/RFSH', desc: 'DRAM refresh' },
      29: GND,
      30: A(0, 'out'), 31: A(1, 'out'), 32: A(2, 'out'), 33: A(3, 'out'), 34: A(4, 'out'), 35: A(5, 'out'), 36: A(6, 'out'), 37: A(7, 'out'), 38: A(8, 'out'), 39: A(9, 'out'), 40: A(10, 'out'),
    }),
    desc: 'The processor. It puts an address on A0–A15, says what it wants with /MREQ or /IORQ and /RD or /WR, and reads or writes a byte on D0–D7. After reset it fetches its first instruction from address 0000h.',
    doc: `${ZILOG}/um0080.pdf`,
  },
  mem32k('rom28c256', '32K EEPROM (28C256)', 'Read-only memory for the program: RAD burns your assembled code into it. On the Easy Z80 the ROM answers at 0000h–7FFFh.', 'https://www.microchip.com/en-us/product/AT28C256'),
  mem32k('ram62256', '32K SRAM (62256)', 'Read/write memory for data and the stack. On the Easy Z80 the RAM answers at 8000h–FFFFh.', 'https://www.alliancememory.com/wp-content/uploads/pdf/AS6C62256.pdf'),
  {
    type: 'sio',
    label: 'Z80 SIO/0 (Z84C40)',
    ref: 'U',
    category: 'Peripherals',
    pkg: { kind: 'dip', pins: 40 },
    pins: pins({
      1: D(1), 2: D(3), 3: D(5), 4: D(7),
      5: { name: '/INT', kind: 'oc', bus: '/INT', desc: 'Interrupt request (open drain)' },
      6: { name: 'IEI', kind: 'in', desc: 'Interrupt enable in (daisy chain)' }, 7: { name: 'IEO', kind: 'out', desc: 'Interrupt enable out (daisy chain)' },
      8: { name: '/M1', kind: 'in', bus: '/M1' }, 9: VCC,
      10: { name: '/W/RDYA', kind: 'out' }, 11: { name: '/SYNCA', kind: 'io' },
      12: { name: 'RxDA', kind: 'in', desc: 'Channel A receive data (from the terminal’s TXD)' },
      13: { name: '/RxCA', kind: 'in', desc: 'Channel A receive clock (1.8432 MHz for 115200 baud)' },
      14: { name: '/TxCA', kind: 'in', desc: 'Channel A transmit clock (1.8432 MHz for 115200 baud)' },
      15: { name: 'TxDA', kind: 'out', desc: 'Channel A transmit data (to the terminal’s RXD)' },
      16: { name: '/DTRA', kind: 'out' }, 17: { name: '/RTSA', kind: 'out' }, 18: { name: '/CTSA', kind: 'in' }, 19: { name: '/DCDA', kind: 'in' },
      20: { name: 'CLK', kind: 'in', bus: 'CLK', desc: 'System clock (the CPU clock)' },
      21: { name: '/RESET', kind: 'in', bus: '/RESET' },
      22: { name: '/DCDB', kind: 'in' }, 23: { name: '/CTSB', kind: 'in' }, 24: { name: '/RTSB', kind: 'out' }, 25: { name: '/DTRB', kind: 'out' },
      26: { name: 'TxDB', kind: 'out', desc: 'Channel B transmit data' }, 27: { name: '/RxTxCB', kind: 'in', desc: 'Channel B clock' }, 28: { name: 'RxDB', kind: 'in', desc: 'Channel B receive data' },
      29: { name: '/SYNCB', kind: 'io' }, 30: { name: '/W/RDYB', kind: 'out' }, 31: GND,
      32: { name: '/RD', kind: 'in', bus: '/RD' },
      33: { name: 'C/D', kind: 'in', bus: 'A0', desc: 'Control (high) or data (low) register: A0 on the Easy Z80' },
      34: { name: 'B/A', kind: 'in', bus: 'A1', desc: 'Channel B (high) or A (low): A1 on the Easy Z80' },
      35: { name: '/CE', kind: 'in', desc: 'Chip enable (ports 80h–83h on the Easy Z80)' },
      36: { name: '/IORQ', kind: 'in', bus: '/IORQ' },
      37: D(6), 38: D(4), 39: D(2), 40: D(0),
    }),
    desc: 'Serial I/O: two serial ports. On the Easy Z80 channel A is the console: write a byte to port 80h to send it, read port 81h to see the status (bit 0: a byte has arrived, bit 2: ready to send).',
    doc: `${ZILOG}/ps0183.pdf`,
  },
  {
    type: 'ctc',
    label: 'Z80 CTC (Z84C30)',
    ref: 'U',
    category: 'Peripherals',
    pkg: { kind: 'dip', pins: 28 },
    pins: pins({
      1: D(4), 2: D(5), 3: D(6), 4: D(7), 5: GND,
      6: { name: '/RD', kind: 'in', bus: '/RD' },
      7: { name: 'ZC/TO0', kind: 'out', desc: 'Channel 0 zero count / timeout pulse' }, 8: { name: 'ZC/TO1', kind: 'out' }, 9: { name: 'ZC/TO2', kind: 'out' },
      10: { name: '/IORQ', kind: 'in', bus: '/IORQ' }, 11: { name: 'IEO', kind: 'out' },
      12: { name: '/INT', kind: 'oc', bus: '/INT', desc: 'Interrupt request (open drain)' }, 13: { name: 'IEI', kind: 'in' },
      14: { name: '/M1', kind: 'in', bus: '/M1' },
      15: { name: 'CLK', kind: 'in', bus: 'CLK', desc: 'System clock: timers count it' },
      16: { name: '/CE', kind: 'in', desc: 'Chip enable (ports 88h–8Bh on the Easy Z80)' },
      17: { name: '/RESET', kind: 'in', bus: '/RESET' },
      18: { name: 'CS0', kind: 'in', bus: 'A0', desc: 'Channel select bit 0' }, 19: { name: 'CS1', kind: 'in', bus: 'A1', desc: 'Channel select bit 1' },
      20: { name: 'CLK/TRG3', kind: 'in' }, 21: { name: 'CLK/TRG2', kind: 'in' }, 22: { name: 'CLK/TRG1', kind: 'in' }, 23: { name: 'CLK/TRG0', kind: 'in' },
      24: VCC, 25: D(0), 26: D(1), 27: D(2), 28: D(3),
    }),
    desc: 'Counter/timer: four channels that count the clock and can interrupt the CPU at a steady rate (interrupt mode 2, with a vector). Ports 88h–8Bh on the Easy Z80.',
    doc: `${ZILOG}/ps0181.pdf`,
  },
  {
    type: 'gal16v8',
    label: 'GAL16V8 (address decoder)',
    ref: 'U',
    category: 'Logic',
    pkg: { kind: 'dip', pins: 20 },
    pins: pins({
      1: { name: 'I1', kind: 'in' }, 2: { name: 'I2', kind: 'in' }, 3: { name: 'I3', kind: 'in' }, 4: { name: 'I4', kind: 'in' }, 5: { name: 'I5', kind: 'in' },
      6: { name: 'I6', kind: 'in' }, 7: { name: 'I7', kind: 'in' }, 8: { name: 'I8', kind: 'in' }, 9: { name: 'I9', kind: 'in' }, 10: GND, 11: { name: 'I11', kind: 'in' },
      12: { name: 'IO12', kind: 'io' }, 13: { name: 'IO13', kind: 'io' }, 14: { name: 'IO14', kind: 'io' }, 15: { name: 'IO15', kind: 'io' },
      16: { name: 'IO16', kind: 'io' }, 17: { name: 'IO17', kind: 'io' }, 18: { name: 'IO18', kind: 'io' }, 19: { name: 'IO19', kind: 'io' }, 20: VCC,
    }),
    desc: 'Programmable logic: you name its pins and write equations, and it becomes the address decoder (the Easy Z80 uses one to make the ROM, RAM, SIO and CTC chip selects). Pins 12–19 with an equation are outputs.',
    doc: 'https://www.microchip.com/en-us/product/ATF16V8B',
    props: {
      names: 'MREQ IORQ WR M1 A2 A3 A4 A5 A6 GND A7 RAMCS NC A15 NC NC ROMCS SIOCS CTCCS VCC',
      equations: '/ROMCS = /MREQ * /A15\n/RAMCS = /MREQ * A15\n/CTCCS = /IORQ * M1 * A7 * /A6 * /A5 * /A4 * A3 * /A2\n/SIOCS = /IORQ * M1 * A7 * /A6 * /A5 * /A4 * /A3 * /A2',
    },
  },
  quad('hc00', '74HC00 (4× NAND)', 'NAND', 'Four NAND gates: the output is low only when both inputs are high.', TI('sn74hc00')),
  quad('hc08', '74HC08 (4× AND)', 'AND', 'Four AND gates.', TI('sn74hc08')),
  quad('hc32', '74HC32 (4× OR)', 'OR', 'Four OR gates: with active-low signals, an OR is an AND of the lows (e.g. /MREQ OR A15 is low only for memory below 8000h).', TI('sn74hc32')),
  {
    type: 'hc04',
    label: '74HC04 (6× NOT)',
    ref: 'U',
    category: 'Logic',
    pkg: { kind: 'dip', pins: 14 },
    pins: pins({ 1: { name: '1A', kind: 'in' }, 2: { name: '1Y', kind: 'out' }, 3: { name: '2A', kind: 'in' }, 4: { name: '2Y', kind: 'out' }, 5: { name: '3A', kind: 'in' }, 6: { name: '3Y', kind: 'out' }, 7: GND, 8: { name: '4Y', kind: 'out' }, 9: { name: '4A', kind: 'in' }, 10: { name: '5Y', kind: 'out' }, 11: { name: '5A', kind: 'in' }, 12: { name: '6Y', kind: 'out' }, 13: { name: '6A', kind: 'in' }, 14: VCC }),
    desc: 'Six inverters.',
    doc: TI('sn74hc04'),
  },
  {
    type: 'hc138',
    label: '74HC138 (3-to-8 decoder)',
    ref: 'U',
    category: 'Logic',
    pkg: { kind: 'dip', pins: 16 },
    pins: pins({
      1: { name: 'A', kind: 'in', desc: 'Select bit 0' }, 2: { name: 'B', kind: 'in', desc: 'Select bit 1' }, 3: { name: 'C', kind: 'in', desc: 'Select bit 2' },
      4: { name: '/G2A', kind: 'in', desc: 'Enable (low)' }, 5: { name: '/G2B', kind: 'in', desc: 'Enable (low)' }, 6: { name: 'G1', kind: 'in', desc: 'Enable (high)' },
      7: { name: '/Y7', kind: 'out' }, 8: GND, 9: { name: '/Y6', kind: 'out' }, 10: { name: '/Y5', kind: 'out' }, 11: { name: '/Y4', kind: 'out' }, 12: { name: '/Y3', kind: 'out' }, 13: { name: '/Y2', kind: 'out' }, 14: { name: '/Y1', kind: 'out' }, 15: { name: '/Y0', kind: 'out' }, 16: VCC,
    }),
    desc: 'Decoder: when enabled, the output numbered by C B A goes low. A classic chip-select maker: e.g. A15–A13 into C B A gives eight 8K blocks.',
    doc: TI('sn74hc138'),
  },
  {
    type: 'hc273',
    label: '74HC273 (8-bit latch)',
    ref: 'U',
    category: 'Output',
    pkg: { kind: 'dip', pins: 20 },
    pins: pins({
      1: { name: '/MR', kind: 'in', bus: '/RESET', desc: 'Master reset: low clears all outputs' },
      2: { name: 'Q0', kind: 'out' }, 3: { name: 'D0', kind: 'in', bus: 'D0' }, 4: { name: 'D1', kind: 'in', bus: 'D1' }, 5: { name: 'Q1', kind: 'out' }, 6: { name: 'Q2', kind: 'out' }, 7: { name: 'D2', kind: 'in', bus: 'D2' }, 8: { name: 'D3', kind: 'in', bus: 'D3' }, 9: { name: 'Q3', kind: 'out' }, 10: GND,
      11: { name: 'CP', kind: 'in', desc: 'Clock: D0–D7 are stored when it goes from low to high (end of an OUT)' },
      12: { name: 'Q4', kind: 'out' }, 13: { name: 'D4', kind: 'in', bus: 'D4' }, 14: { name: 'D5', kind: 'in', bus: 'D5' }, 15: { name: 'Q5', kind: 'out' }, 16: { name: 'Q6', kind: 'out' }, 17: { name: 'D6', kind: 'in', bus: 'D6' }, 18: { name: 'D7', kind: 'in', bus: 'D7' }, 19: { name: 'Q7', kind: 'out' }, 20: VCC,
    }),
    desc: 'Eight D flip-flops: an output port. Clock it with a chip select made from /IORQ and /WR, and OUT (port),A shows the byte on Q0–Q7 (LEDs!).',
    doc: TI('sn74hc273'),
  },
  {
    type: 'hc245',
    label: '74HC245 (bus buffer)',
    ref: 'U',
    category: 'Logic',
    pkg: { kind: 'dip', pins: 20 },
    pins: pins({
      1: { name: 'DIR', kind: 'in', desc: 'High: A to B. Low: B to A' },
      2: { name: 'A1', kind: 'io' }, 3: { name: 'A2', kind: 'io' }, 4: { name: 'A3', kind: 'io' }, 5: { name: 'A4', kind: 'io' }, 6: { name: 'A5', kind: 'io' }, 7: { name: 'A6', kind: 'io' }, 8: { name: 'A7', kind: 'io' }, 9: { name: 'A8', kind: 'io' }, 10: GND,
      11: { name: 'B8', kind: 'io' }, 12: { name: 'B7', kind: 'io' }, 13: { name: 'B6', kind: 'io' }, 14: { name: 'B5', kind: 'io' }, 15: { name: 'B4', kind: 'io' }, 16: { name: 'B3', kind: 'io' }, 17: { name: 'B2', kind: 'io' }, 18: { name: 'B1', kind: 'io' },
      19: { name: '/OE', kind: 'in', desc: 'Output enable (low)' }, 20: VCC,
    }),
    desc: 'Eight-bit buffer: connects the A side to the B side in the direction DIR says while /OE is low. With switches on the B side it makes an input port.',
    doc: TI('sn74hc245'),
  },
  {
    type: 'osc',
    label: 'Crystal oscillator',
    ref: 'X',
    category: 'Clock & reset',
    pkg: { kind: 'dip', pins: 8 },
    pins: pins({ 1: { name: 'EN', kind: 'in', desc: 'Enable (high or not connected: running)' }, 4: GND, 5: { name: 'OUT', kind: 'out', bus: 'CLK', desc: 'The clock signal' }, 8: VCC }),
    desc: 'A crystal oscillator can: makes the clock. The Easy Z80 has 10 MHz for the CPU and 1.8432 MHz for the serial ports.',
    props: { hz: 4_000_000 },
  },
  {
    type: 'button',
    label: 'Push button',
    ref: 'SW',
    category: 'Clock & reset',
    pkg: { kind: 'two' },
    pins: pins({ 1: { name: '1', kind: 'pas' }, 2: { name: '2', kind: 'pas' } }),
    desc: 'Connects its two pins while you hold it (click it while running). Between /RESET and GND it is the reset button.',
  },
  {
    type: 'switch',
    label: 'Toggle switch',
    ref: 'SW',
    category: 'Clock & reset',
    pkg: { kind: 'two' },
    pins: pins({ 1: { name: '1', kind: 'pas' }, 2: { name: '2', kind: 'pas' } }),
    desc: 'Connects its two pins when on (click it to flip it).',
    props: { on: false },
  },
  {
    type: 'resistor',
    label: 'Resistor',
    ref: 'R',
    category: 'Passive',
    pkg: { kind: 'two' },
    pins: pins({ 1: { name: '1', kind: 'pas' }, 2: { name: '2', kind: 'pas' } }),
    desc: 'Limits current. To VCC it is a pull-up: it holds an input high (1) unless something drives it low. In series with an LED it stops the LED burning out.',
    props: { ohms: 10_000 },
  },
  {
    type: 'rnet',
    label: 'Resistor network (8× 4.7k)',
    ref: 'RN',
    category: 'Passive',
    pkg: { kind: 'box', w: 60, h: 190 },
    pins: pins({ 1: { name: 'COM', kind: 'pas', bus: 'VCC', desc: 'Common: to VCC for pull-ups' }, 2: { name: 'R1', kind: 'pas' }, 3: { name: 'R2', kind: 'pas' }, 4: { name: 'R3', kind: 'pas' }, 5: { name: 'R4', kind: 'pas' }, 6: { name: 'R5', kind: 'pas' }, 7: { name: 'R6', kind: 'pas' }, 8: { name: 'R7', kind: 'pas' }, 9: { name: 'R8', kind: 'pas' } }),
    desc: 'Eight resistors joined at pin 1 (SIP-9, bussed). With COM on VCC it pulls up eight lines at once, like the data bus pull-ups on the Easy Z80.',
    props: { ohms: 4700 },
  },
  {
    type: 'capacitor',
    label: 'Capacitor',
    ref: 'C',
    category: 'Passive',
    pkg: { kind: 'two' },
    pins: pins({ 1: { name: '1', kind: 'pas' }, 2: { name: '2', kind: 'pas' } }),
    desc: 'Stores charge. 100 nF between VCC and GND next to each chip smooths its supply (decoupling); on /RESET with a pull-up it makes a power-on reset.',
    props: { farads: 1e-7 },
  },
  {
    type: 'led',
    label: 'LED',
    ref: 'D',
    category: 'Output',
    pkg: { kind: 'two' },
    pins: pins({ 1: { name: 'A', kind: 'pas', desc: 'Anode (+)' }, 2: { name: 'K', kind: 'pas', desc: 'Cathode (−, the flat side)' } }),
    desc: 'Lights when current flows from anode to cathode. Always put a resistor (about 470 Ω) in series.',
    props: { color: 'red' },
  },
  {
    type: 'ledbar',
    label: 'LED bar (8 LEDs + resistors)',
    ref: 'D',
    category: 'Output',
    pkg: { kind: 'box', w: 70, h: 190 },
    pins: pins({ 1: { name: 'L0', kind: 'in' }, 2: { name: 'L1', kind: 'in' }, 3: { name: 'L2', kind: 'in' }, 4: { name: 'L3', kind: 'in' }, 5: { name: 'L4', kind: 'in' }, 6: { name: 'L5', kind: 'in' }, 7: { name: 'L6', kind: 'in' }, 8: { name: 'L7', kind: 'in' }, 9: { name: 'GND', kind: 'gnd', bus: 'GND' } }),
    desc: 'Eight LEDs, each with its own 470 Ω resistor to GND: a high input lights it. Wire it to a 74HC273’s outputs to see a byte.',
  },
  {
    type: 'terminal',
    label: 'Serial terminal (USB-TTL)',
    ref: 'J',
    category: 'Peripherals',
    pkg: { kind: 'box', w: 90, h: 70 },
    pins: pins({ 1: { name: 'RXD', kind: 'in', desc: 'Receives: wire it to the SIO’s TxDA' }, 2: { name: 'TXD', kind: 'out', desc: 'Sends what you type: wire it to the SIO’s RxDA' }, 3: { name: 'GND', kind: 'gnd', bus: 'GND' } }),
    desc: 'A USB-to-serial adapter to this computer: what the SIO sends appears in the Terminal tab, and what you type there is sent to it.',
  },
  {
    type: 'psu',
    label: 'Power supply (5 V)',
    ref: 'PS',
    category: 'Power',
    pkg: { kind: 'box', w: 80, h: 60 },
    pins: pins({ 1: { name: '+5V', kind: 'out', bus: 'VCC', desc: '+5 V: to every VCC pin' }, 2: { name: 'GND', kind: 'out', bus: 'GND', desc: '0 V: to every GND pin' } }),
    desc: 'The regulated 5 V supply (the Easy Z80’s barrel jack). Every chip needs VCC and GND wired to it.',
  },
];

export const PART = Object.fromEntries(PARTS.map((p) => [p.type, p])) as Record<string, PartDef>;
export const CATEGORIES: Category[] = ['Power', 'Processor', 'Memory', 'Peripherals', 'Logic', 'Clock & reset', 'Passive', 'Output'];

// ------------------------------------------------------------------ geometry

export const PITCH = 20;
export const STUB = 14;

/** A part's body size. */
export function bodySize(def: PartDef): { w: number; h: number } {
  if (def.pkg.kind === 'dip') {
    const n = def.pkg.pins;
    return { w: n >= 40 ? 150 : n >= 28 ? 130 : n >= 20 ? 110 : 90, h: (n / 2) * PITCH + PITCH };
  }
  if (def.pkg.kind === 'two') return { w: 60, h: 24 };
  return { w: def.pkg.w, h: def.pkg.h };
}

/** Where a pin's end is, from the part's top left, and which way the pin points. */
export function pinPos(def: PartDef, n: number, rot = 0): { x: number; y: number; dx: number; dy: number } {
  const { w, h } = bodySize(def);
  let p: { x: number; y: number; dx: number; dy: number };
  if (def.pkg.kind === 'dip') {
    const half = def.pkg.pins / 2;
    p = n <= half ? { x: -STUB, y: PITCH + (n - 1) * PITCH, dx: -1, dy: 0 } : { x: w + STUB, y: PITCH + (def.pkg.pins - n) * PITCH, dx: 1, dy: 0 };
  } else if (def.pkg.kind === 'two') {
    p = n === 1 ? { x: -STUB, y: h / 2, dx: -1, dy: 0 } : { x: w + STUB, y: h / 2, dx: 1, dy: 0 };
  } else {
    const i = def.pins.findIndex((x) => x.n === n);
    p = { x: w + STUB, y: PITCH + i * PITCH, dx: 1, dy: 0 };
  }
  if (rot === 90 && def.pkg.kind === 'two') {
    // Turned a quarter: pin 1 on top, pin 2 at the bottom (about the body's centre).
    const cx = w / 2;
    const cy = h / 2;
    return { x: cx - (p.y - cy), y: cy + (p.x - cx), dx: -p.dy, dy: p.dx };
  }
  return p;
}

/** "U1.6 (CLK)". */
export const pinLabel = (ref: string, def: PartDef | undefined, n: number) => {
  const pin = def?.pins.find((p) => p.n === n);
  return `${ref}.${n}${pin && pin.name !== String(n) ? ` (${pin.name})` : ''}`;
};
