//! Provides facilities for examining scripts to determine their compatibility with iOS.

use byteorder::{LittleEndian, ReadBytesExt};
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Display;
use std::io::{self, Cursor, Error, ErrorKind, Seek};

#[derive(Debug, Clone)]
struct Variable {
    value: i64,
    location: Location,
}

impl std::fmt::Display for Variable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let prefix = match self.location {
            Location::Global => "global",
            Location::Local => "local",
            _ => panic!("Invalid variable location"),
        };

        f.write_fmt(format_args!("{}_{:#x}", prefix, self.value))
    }
}

#[derive(Debug, Clone)]
struct Array;

#[derive(Debug, Clone)]
struct Pointer(i64);

impl Pointer {
    fn from_i64(i: i64) -> Pointer {
        Pointer(i)
    }

    fn is_local(&self) -> bool {
        self.0 < 0
    }

    fn absolute(&self) -> u64 {
        self.0.abs() as u64
    }
}

impl std::fmt::Display for Pointer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_local() {
            write!(f, "{:#x}", self.absolute())
        } else {
            f.write_fmt(format_args!("Global({:#x})", self.absolute()))
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
enum Value {
    Integer(i64),
    Real(f32),
    String(String),
    Model(i64),
    Pointer(Pointer),
    VarArgs(Vec<Value>),
    Buffer(String),
    Variable(Variable),
    Array(Array),
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Integer(int) => int.fmt(f),
            Value::Real(real) => f.write_fmt(format_args!("{}f", real)),
            Value::String(string) => f.write_fmt(format_args!("\"{}\"", string)),
            Value::Pointer(pointer) => pointer.fmt(f),
            Value::Variable(var) => var.fmt(f),
            Value::Array(_) => f.write_str("arr"),
            _ => panic!("wtf"),
        }
    }
}

impl Value {
    fn read(reader: &mut impl io::Read) -> io::Result<Value> {
        let id = reader.read_u8()?;

        Ok(match id {
            0x1 => Value::Integer(reader.read_i32::<LittleEndian>()? as i64),
            0x2 => Value::Variable(Variable {
                value: reader.read_u16::<LittleEndian>()? as i64,
                location: Location::Global,
            }),
            0x3 => Value::Variable(Variable {
                value: reader.read_u16::<LittleEndian>()? as i64,
                location: Location::Local,
            }),
            0x4 => Value::Integer(reader.read_i8()? as i64),
            0x5 => Value::Integer(reader.read_i16::<LittleEndian>()? as i64),
            0x6 => Value::Real(reader.read_f32::<LittleEndian>()?),
            0x7 => {
                reader.read_exact(&mut [0; 6])?;
                Value::Array(Array {})
            }
            0x8 => {
                reader.read_exact(&mut [0; 6])?;
                Value::Array(Array {})
            }
            0x9 => {
                let mut buf = [0u8; 8];
                reader.read_exact(&mut buf[..])?;
                Value::String(
                    buf.iter()
                        .take_while(|v| v != &&0)
                        .map(|v| *v as char)
                        .collect::<String>(),
                )
            }
            0xa => Value::Variable(Variable {
                value: reader.read_u16::<LittleEndian>()? as i64,
                location: Location::Global,
            }),
            0xb => Value::Variable(Variable {
                value: reader.read_u16::<LittleEndian>()? as i64,
                location: Location::Local,
            }),
            0xc => {
                reader.read_exact(&mut [0; 6])?;
                Value::Array(Array {})
            }
            0xd => {
                reader.read_exact(&mut [0; 6])?;
                Value::Array(Array {})
            }
            0xe => {
                let length = reader.read_u8()? as usize;
                let mut vec: Vec<u8> = std::iter::repeat(0u8).take(length).collect();
                reader.read_exact(vec.as_mut_slice())?;
                Value::String(
                    vec.iter()
                        .take_while(|v| v != &&0)
                        .map(|v| *v as char)
                        .collect::<String>(),
                )
            }
            0xf => {
                let mut buf = [0u8; 16];
                reader.read_exact(&mut buf[..])?;
                Value::String(
                    buf.iter()
                        .take_while(|v| v != &&0)
                        .map(|v| *v as char)
                        .collect::<String>(),
                )
            }
            0x10 => Value::Variable(Variable {
                value: reader.read_u16::<LittleEndian>()? as i64,
                location: Location::Global,
            }),
            0x11 => Value::Variable(Variable {
                value: reader.read_u16::<LittleEndian>()? as i64,
                location: Location::Local,
            }),
            0x12 => {
                reader.read_exact(&mut [0; 6])?;
                Value::Array(Array {})
            }
            0x13 => {
                reader.read_exact(&mut [0; 6])?;
                Value::Array(Array {})
            }

            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Unknown type ID '{}'", id),
                ));
            }
        })
    }
}

#[derive(Debug, Deserialize, Serialize)]
enum ParamType {
    /// An integral value.
    Integer,

    /// A string value.
    String,

    /// A floating-point value.
    Real,

    /// A model ID.
    Model,

    /// A pointer to a script location.
    Pointer,

    /// A null byte to mark the end of argument lists.
    End,

    /// Long buffer; used only for opcode 05B6.
    Buffer,

    /// Allows any type. Typically used for variadic arguments.
    Any,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy)]
enum Location {
    /// A value directly written in the instruction bytecode.
    Immediate,

    /// A variable local to the script.
    Local,

    /// A variable shared between scripts.
    Global,
}

#[derive(Debug, Deserialize, Serialize)]
struct Param {
    param_type: ParamType,
    location: Location,
    is_variadic: bool,
    is_output: bool,
}

impl Param {
    fn read(&self, reader: &mut impl io::Read) -> io::Result<Value> {
        let value = Value::read(reader)?;

        if let ParamType::Pointer = self.param_type {
            if let Value::Integer(int) = value {
                return Ok(Value::Pointer(Pointer::from_i64(int)));
            }
        }

        Ok(value)
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct Command {
    opcode: u16,
    name: String,
    returns: bool,
    params: Vec<Param>,
}

fn load_all_commands() -> std::result::Result<HashMap<u16, Command>, Box<bincode::ErrorKind>> {
    let commands_bin = include_bytes!("../commands.bin");
    bincode::deserialize(commands_bin)
}

struct Instr {
    opcode: u16,
    name: String,

    offset: u64,

    bool_inverted: bool,
    args: Vec<Value>,
}

impl Instr {
    fn read(commands: &HashMap<u16, Command>, reader: &mut Cursor<&[u8]>) -> io::Result<Instr> {
        let offset = reader.position();

        let (opcode, bool_inverted) = {
            let opcode_in_file = reader.read_u16::<LittleEndian>()?;

            // The most significant bit (0x8000) is set when the returned
            //  boolean is to be inverted.
            (opcode_in_file & 0x7fff, opcode_in_file & 0x8000 != 0)
        };

        let cmd = match commands.get(&opcode) {
            Some(command) => command,
            None => {
                // If we don't know the opcode, then we can't get the parameter list,
                //  which is necessary for reading the instruction.
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    format!("unknown opcode {:#x}", opcode),
                ));
            }
        };

        let mut args = Vec::with_capacity(cmd.params.len());

        for param in cmd.params.iter() {
            args.push(param.read(reader)?);
        }

        Ok(Instr {
            opcode,
            name: cmd.name.clone(),
            offset,
            bool_inverted,
            args,
        })
    }

    fn next_offsets(&self, current: u64, offsets: &mut Vec<u64>) {
        // The 'return' command should go to the return address on the call stack,
        //  but we already handle that case when we branch at 'gosub'.
        if self.opcode == 0x0051 {
            return;
        }

        // goto always branches. Everything else is assumed to also go onto the next instruction.
        if self.opcode != 0x0002 {
            offsets.push(current);
        }

        // goto, goto_if_true, goto_if_false, gosub, switch_start and switch_continue can all
        //  branch to every pointer they reference (in theory).
        if let 0x0002 | 0x004c | 0x004d | 0x0050 | 0x0871 | 0x0872 = self.opcode {
            for arg in &self.args {
                if let Value::Pointer(ptr) = arg {
                    if ptr.is_local() {
                        offsets.push(ptr.absolute());
                    }
                }
            }
        }
    }
}

impl Display for Instr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:08x} {:04x} {}{}({})",
            self.offset,
            self.opcode,
            if self.bool_inverted { "!" } else { "" },
            self.name,
            self.args
                .iter()
                .map(Value::to_string)
                .collect::<Vec<String>>()
                .join(", ")
        )
    }
}

fn disassemble(
    commands: &HashMap<u16, Command>,
    reader: &mut Cursor<&[u8]>,
    instrs: &mut HashMap<u64, Instr>,
) -> io::Result<()> {
    let start = std::time::Instant::now();

    // Start with offset 0 (the beginning of the script).
    let mut cur_offsets: Vec<u64> = vec![0];

    // We only use this vector inside the `while` loop, but we create it here so fewer
    //  allocations take place (since it keeps its buffer in between iterations).
    let mut new_offsets: Vec<u64> = Vec::new();

    while !cur_offsets.is_empty() {
        for offset in cur_offsets.iter() {
            if instrs.contains_key(offset) {
                continue;
            }

            reader.seek(io::SeekFrom::Start(*offset))?;

            let instr = match Instr::read(commands, reader) {
                Ok(instr) => instr,
                Err(err) => {
                    // Log the error and continue - we can't guarantee that the game would go down
                    //  this invalid path, so the script could still run fine (this is the basis
                    //  of many script obfuscation methods).
                    log::warn!("encountered error at {:#x}: {}", *offset, err);

                    continue;
                }
            };

            instr.next_offsets(reader.position(), &mut new_offsets);
            instrs.insert(*offset, instr);
        }

        cur_offsets.clear();
        cur_offsets.append(&mut new_offsets);
    }

    let end = std::time::Instant::now();
    let time_taken = end - start;

    log::info!("disassembly took {:#?}", time_taken);

    Ok(())
}

fn get_commands() -> &'static HashMap<u16, Command> {
    static COMMANDS_CELL: OnceCell<HashMap<u16, Command>> = OnceCell::new();

    COMMANDS_CELL.get_or_init(|| {
        let loaded = match load_all_commands() {
            Ok(l) => l,
            Err(err) => {
                log::error!("error loading commands: {}", err);
                return HashMap::new();
            }
        };

        loaded
    })
}

/// Defines reasons why a script should be marked as potentially incompatible.
#[derive(Debug)]
pub enum CompatIssue {
    /// The script relies on Android-specific stuff such as hardcoded memory addresses or symbol names.
    AndroidSpecific,

    /// CLEO does not yet implement a particular command that the script uses.
    NotImpl,

    /// We can't say either way if the script is compatible, because the check failed for some reason.
    CheckFailed,
}

impl Display for CompatIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AndroidSpecific => f.write_str("Script uses Android-specific code."),
            Self::NotImpl => f.write_str("Script uses features not yet present on iOS."),
            Self::CheckFailed => f.write_str("Unable to complete script check on this script."),
        }
    }
}

pub fn check_bytecode(bytes: &[u8]) -> Result<Option<CompatIssue>, String> {
    // Even though we don't particularly care about the offsets, we need a HashMap so that `disassemble` can
    //  easily check if it's visited an offset before (to avoid infinite loops).
    let mut instruction_map = HashMap::new();

    let disasm_result = disassemble(
        get_commands(),
        &mut Cursor::new(bytes),
        &mut instruction_map,
    );

    if let Err(err) = disasm_result {
        log::warn!("error at end of disassembly: {}", err);
    } else {
        log::info!("finished disassembly");
    }

    for (_, instr) in instruction_map.iter() {
        match instr.opcode {
            0x0dd5 | 0x0dd6 | 0x0de1..=0x0df6 => return Ok(Some(CompatIssue::NotImpl)),
            0x0dd0..=0x0ddb | 0x0dde => return Ok(Some(CompatIssue::AndroidSpecific)),

            _ => (),
        }
    }

    Ok(None)
}
