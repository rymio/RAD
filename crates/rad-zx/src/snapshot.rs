//! Z80 and SNA snapshots: a saved machine state, loaded through SPECTRUSTY's snapshot readers.

use std::io::{Cursor, Read};

use spectrusty::chip::{ControlUnit, EarIn, FrameState, MemoryAccess, UlaControl};
use spectrusty::formats::snapshot::{Ay3_891xDevice, ComputerModel, CpuModel, Extensions, MemoryRange, SnapshotLoader};
use spectrusty::memory::{ZxMemory, ZxMemoryError};
use spectrusty::peripherals::ay::AyRegister;
use spectrusty::video::{BorderColor, Video};
use spectrusty::z80emu::Z80NMOS;
use spectrusty::chip::ReadEarMode;

use crate::{machine_128, machine_48, Machine, Ula128MemFlags};

struct Loader {
    m: Option<Machine>,
}

impl SnapshotLoader for Loader {
    type Error = String;

    fn select_model(&mut self, model: ComputerModel, extensions: Extensions, border: BorderColor, issue: ReadEarMode) -> Result<(), String> {
        if extensions != Extensions::NONE {
            return Err(format!("it needs hardware RAD doesn't emulate ({extensions:?})"));
        }
        let mut m = match model {
            ComputerModel::Spectrum16 | ComputerModel::Spectrum48 => Machine::S48(machine_48()),
            ComputerModel::Spectrum128 | ComputerModel::SpectrumPlus2 => Machine::S128(machine_128()),
            other => return Err(format!("it's for a {other:?}, which RAD doesn't emulate yet")),
        };
        with_zx!(&mut m, zx => {
            zx.ula.set_border_color(border);
            zx.ula.set_read_ear_mode(issue);
        });
        self.m = Some(m);
        Ok(())
    }

    fn read_into_memory<R: Read>(&mut self, range: MemoryRange, mut reader: R) -> Result<(), ZxMemoryError> {
        let Some(m) = self.m.as_mut() else { return Ok(()) };
        match range {
            MemoryRange::Ram(r) => with_zx!(m, zx => {
                let ram = zx.ula.memory_mut().ram_mut();
                let dest = ram.get_mut(r).ok_or(ZxMemoryError::InvalidPageIndex)?;
                reader.read_exact(dest).map_err(ZxMemoryError::Io)
            }),
            _ => {
                // A custom ROM or interface ROM: keep the machine's own.
                let _ = std::io::copy(&mut reader, &mut std::io::sink());
                Ok(())
            }
        }
    }

    fn assign_cpu(&mut self, cpu: CpuModel) {
        if let Some(m) = self.m.as_mut() {
            with_zx!(m, zx => zx.cpu = Z80NMOS::from(cpu));
        }
    }

    fn set_clock(&mut self, tstates: spectrusty::clock::FTs) {
        if let Some(m) = self.m.as_mut() {
            with_zx!(m, zx => zx.ula.set_frame_tstate(tstates));
        }
    }

    fn write_port(&mut self, port: u16, data: u8) {
        let Some(m) = self.m.as_mut() else { return };
        if port & 1 == 0 {
            if let Ok(c) = BorderColor::try_from(data & 7) {
                with_zx!(m, zx => zx.ula.set_border_color(c));
            }
        }
        if let Machine::S128(zx) = m {
            if port & 0x8002 == 0 {
                zx.ula.set_ula128_mem_port_value(Ula128MemFlags::from_bits_truncate(data));
            }
        }
    }

    fn setup_ay(&mut self, _choice: Ay3_891xDevice, reg_selected: AyRegister, reg_values: &[u8; 16]) {
        if let Some(Machine::S128(zx)) = self.m.as_mut() {
            let ay = zx.ula.bus_device_mut();
            for (r, v) in reg_values.iter().enumerate() {
                ay.ay_sound.update_register(AyRegister::from(r as u8), *v);
                ay.ay_io.select_port_write(r as u8);
                ay.ay_io.data_port_write(0xbffd, *v, 0);
            }
            ay.ay_io.select_port_write(u8::from(reg_selected));
        }
    }
}

/// Loads a Z80 or SNA snapshot into a new machine of the model it was made on.
pub(crate) fn load(ext: &str, data: &[u8]) -> Result<Machine, String> {
    let mut l = Loader { m: None };
    let r = match ext {
        "z80" => spectrusty::formats::z80::load_z80(Cursor::new(data), &mut l),
        _ => spectrusty::formats::sna::load_sna(Cursor::new(data), &mut l),
    };
    r.map_err(|e| e.to_string())?;
    l.m.ok_or_else(|| "it's empty".to_string())
}

// ------------------------------------------------------------------ saving (save states)

use spectrusty::formats::snapshot::{JoystickModel, SnapshotCreator};

struct Creator<'a>(&'a Machine);

impl SnapshotCreator for Creator<'_> {
    fn model(&self) -> ComputerModel {
        match self.0 {
            Machine::S48(_) => ComputerModel::Spectrum48,
            Machine::S128(_) => ComputerModel::Spectrum128,
        }
    }
    fn extensions(&self) -> Extensions {
        Extensions::NONE
    }
    fn cpu(&self) -> CpuModel {
        with_zx!(self.0, zx => CpuModel::NMOS(zx.cpu.clone()))
    }
    fn current_clock(&self) -> spectrusty::clock::FTs {
        with_zx!(self.0, zx => zx.ula.current_tstate())
    }
    fn border_color(&self) -> BorderColor {
        with_zx!(self.0, zx => zx.ula.border_color())
    }
    fn issue(&self) -> ReadEarMode {
        with_zx!(self.0, zx => zx.ula.read_ear_mode())
    }
    fn memory_ref(&self, range: MemoryRange) -> Result<&[u8], ZxMemoryError> {
        with_zx!(self.0, zx => {
            let mem = zx.ula.memory_ref();
            match range {
                MemoryRange::Ram(r) => mem.ram_ref().get(r).ok_or(ZxMemoryError::InvalidPageIndex),
                MemoryRange::Rom(r) => mem.rom_ref().get(r).ok_or(ZxMemoryError::InvalidPageIndex),
                _ => Err(ZxMemoryError::UnsupportedAddressRange),
            }
        })
    }
    fn joystick(&self) -> Option<JoystickModel> {
        Some(JoystickModel::Kempston)
    }
    fn ay_state(&self, _choice: Ay3_891xDevice) -> Option<(AyRegister, &[u8; 16])> {
        match self.0 {
            Machine::S128(zx) => {
                let ay = &zx.ula.bus_device_ref().ay_io;
                Some((ay.selected_register(), ay.registers()))
            }
            Machine::S48(_) => None,
        }
    }
    fn ula128_flags(&self) -> Ula128MemFlags {
        match self.0 {
            Machine::S128(zx) => zx.ula.ula128_mem_port_value().unwrap_or_default(),
            Machine::S48(_) => Ula128MemFlags::empty(),
        }
    }
}

/// The machine as a Z80 (v3) snapshot: a save state any Spectrum emulator can open.
pub(crate) fn save(m: &Machine) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    spectrusty::formats::z80::save_z80v3(&Creator(m), &mut out).map_err(|e| e.to_string())?;
    Ok(out)
}
