#![allow(non_upper_case_globals)]

use crate::alloc::alloc_blob;
use crate::buf::{read_byte, read_word, skip_leb128, Buf};
use crate::leb128::{leb128_decode, sleb128_decode};
use crate::trap_with_prefix;
use crate::types::Words;
use crate::utf8::utf8_validate;

//
// IDL constants
//

const IDL_PRIM_null: i32 = -1;
const IDL_PRIM_bool: i32 = -2;
const IDL_PRIM_nat: i32 = -3;
const IDL_PRIM_int: i32 = -4;
const IDL_PRIM_nat8: i32 = -5;
const IDL_PRIM_nat16: i32 = -6;
const IDL_PRIM_nat32: i32 = -7;
const IDL_PRIM_nat64: i32 = -8;
const IDL_PRIM_int8: i32 = -9;
const IDL_PRIM_int16: i32 = -10;
const IDL_PRIM_int32: i32 = -11;
const IDL_PRIM_int64: i32 = -12;
const IDL_PRIM_float32: i32 = -13;
const IDL_PRIM_float64: i32 = -14;
const IDL_PRIM_text: i32 = -15;
const IDL_PRIM_reserved: i32 = -16;
const IDL_PRIM_empty: i32 = -17;

const IDL_CON_opt: i32 = -18;
const IDL_CON_vec: i32 = -19;
const IDL_CON_record: i32 = -20;
const IDL_CON_variant: i32 = -21;
const IDL_CON_func: i32 = -22;
const IDL_CON_service: i32 = -23;

const IDL_REF_principal: i32 = -24;

const IDL_CON_alias: i32 = 1;

const IDL_PRIM_lowest: i32 = -17;

pub(crate) unsafe fn idl_trap_with(msg: &str) -> ! {
    trap_with_prefix("IDL error: ", msg);
}

unsafe fn is_primitive_type(ty: i32) -> bool {
    ty < 0 && (ty >= IDL_PRIM_lowest || ty == IDL_REF_principal)
}

unsafe fn check_typearg(ty: i32, n_types: u32) {
    // Arguments to type constructors can be primitive types or type indices
    if !(is_primitive_type(ty) || (ty >= 0 && (ty as u32) < n_types)) {
        idl_trap_with("invalid type argument");
    }
}

unsafe fn parse_fields(buf: *mut Buf, n_types: u32) {
    let mut next_valid = 0;
    for n in (1..=leb128_decode(buf)).rev() {
        let tag = leb128_decode(buf);
        if (tag < next_valid) || (tag == 0xFFFFFFFF && n > 1) {
            idl_trap_with("variant or record tag out of order");
        }
        next_valid = tag + 1;
        let t = sleb128_decode(buf);
        check_typearg(t, n_types);
    }
}

// NB. This function assumes the allocation does not need to survive GC
unsafe fn alloc(size: Words<u32>) -> *mut u8 {
    alloc_blob(size.to_bytes()).as_blob().payload_addr()
}

/// This function parses the IDL magic header and type description. It
///
/// * traps if the type description is not well-formed. In particular, it traps if any index into
///   the type description table is out of bounds, so that subsequent code can trust these values
///
/// * returns a pointer to the first byte after the IDL header
///
/// * allocates a type description table, and returns it
///   (via pointer argument, for lack of multi-value returns in C ABI)
///
/// * returns the size of that type description table
///   (again via pointer argument, for lack of multi-value returns in C ABI)
///
/// * returns a pointer to the beginning of the list of main types
///   (again via pointer argument, for lack of multi-value returns in C ABI)
#[no_mangle]
unsafe extern "C" fn parse_idl_header(
    extended: bool,
    buf: *mut Buf,
    typtbl_out: *mut *mut *mut u8,
    typtbl_size_out: *mut u32,
    main_types_out: *mut *mut u8,
) {
    if (*buf).ptr == (*buf).end {
        idl_trap_with("empty input");
    }

    // Magic bytes (DIDL)
    if read_word(buf) != 0x4C444944 {
        idl_trap_with("missing magic bytes");
    }

    // Create a table for the type description
    let n_types = leb128_decode(buf);

    // Early sanity check
    if (*buf).ptr.add(n_types as usize) >= (*buf).end {
        idl_trap_with("too many types");
    }

    // Let the caller know about the table size
    *typtbl_size_out = n_types;

    // Go through the table
    let typtbl: *mut *mut u8 = alloc(Words(n_types)) as *mut _;

    for i in 0..n_types {
        *typtbl.add(i as usize) = (*buf).ptr;

        let ty = sleb128_decode(buf);

        if extended && ty == IDL_CON_alias {
            // internal
            // See Note [mutable stable values] in codegen/compile.ml
            let t = sleb128_decode(buf);
            check_typearg(t, n_types);
        } else if ty >= 0 {
            idl_trap_with("illegal type table"); // illegal
        } else if is_primitive_type(ty) {
            // illegal
            idl_trap_with("primitive type in type table");
        } else if ty == IDL_CON_opt {
            let t = sleb128_decode(buf);
            check_typearg(t, n_types);
        } else if ty == IDL_CON_vec {
            let t = sleb128_decode(buf);
            check_typearg(t, n_types);
        } else if ty == IDL_CON_record {
            parse_fields(buf, n_types);
        } else if ty == IDL_CON_variant {
            parse_fields(buf, n_types);
        } else if ty == IDL_CON_func {
            // Arg types
            for _ in 0..leb128_decode(buf) {
                let t = sleb128_decode(buf);
                check_typearg(t, n_types);
            }
            // Ret types
            for _ in 0..leb128_decode(buf) {
                let t = sleb128_decode(buf);
                check_typearg(t, n_types);
            }
            // Annotations
            for _ in 0..leb128_decode(buf) {
                buf.advance(1)
            }
        } else if ty == IDL_CON_service {
            for _ in 0..leb128_decode(buf) {
                // Name
                let size = leb128_decode(buf);
                buf.advance(size);
                // Type
                let t = sleb128_decode(buf);
                check_typearg(t, n_types);
            }
        } else {
            // Future type
            let n = leb128_decode(buf);
            buf.advance(n);
        }
    }

    // Now read the main types
    *main_types_out = (*buf).ptr;
    for _ in 0..leb128_decode(buf) {
        let t = sleb128_decode(buf);
        check_typearg(t, n_types);
    }

    *typtbl_out = typtbl;
}

// used for opt, bool, references...
unsafe fn read_byte_tag(buf: *mut Buf) -> u8 {
    let b = read_byte(buf);
    if b > 1 {
        idl_trap_with("skip_any: byte tag not 0 or 1");
    }
    b
}

// Assumes buf is the encoding of type t, and fast-forwards past that
// Assumes all type references in the typtbl are already checked
//
// This is currently implemented recursively, but we could
// do this in a loop (by maintaing a stack of the t arguments)
#[no_mangle]
unsafe extern "C" fn skip_any(buf: *mut Buf, typtbl: *mut *mut u8, t: i32, depth: i32) {
    if depth > 100 {
        idl_trap_with("skip_any: too deeply nested record");
    }

    if t < 0 {
        // Primitive type
        match t {
            IDL_PRIM_null | IDL_PRIM_reserved => {}
            IDL_PRIM_bool => {
                read_byte_tag(buf);
            }
            IDL_PRIM_nat | IDL_PRIM_int => {
                skip_leb128(buf);
            }
            IDL_PRIM_nat8 | IDL_PRIM_int8 => {
                buf.advance(1);
            }
            IDL_PRIM_nat16 | IDL_PRIM_int16 => {
                buf.advance(2);
            }
            IDL_PRIM_nat32 | IDL_PRIM_int32 | IDL_PRIM_float32 => {
                buf.advance(4);
            }
            IDL_PRIM_nat64 | IDL_PRIM_int64 | IDL_PRIM_float64 => {
                buf.advance(8);
            }
            IDL_PRIM_text => {
                let len = leb128_decode(buf);
                let p = (*buf).ptr;
                buf.advance(len); // advance first; does the bounds check
                utf8_validate(p as *const _, len);
            }
            IDL_PRIM_empty => {
                idl_trap_with("skip_any: encountered empty");
            }
            IDL_REF_principal => {
                if read_byte_tag(buf) != 0 {
                    let len = leb128_decode(buf);
                    buf.advance(len);
                }
            }
            _ => {
                idl_trap_with("skip_any: unknown prim");
            }
        }
    } else {
        // t >= 0
        let mut tb = Buf {
            ptr: *typtbl.add(t as usize),
            end: (*buf).end,
        };
        let tc = sleb128_decode(&mut tb);
        match tc {
            IDL_CON_opt => {
                let it = sleb128_decode(&mut tb);
                if read_byte_tag(buf) != 0 {
                    skip_any(buf, typtbl, it, 0);
                }
            }
            IDL_CON_vec => {
                let it = sleb128_decode(&mut tb);
                for _ in 0..leb128_decode(buf) {
                    skip_any(buf, typtbl, it, 0);
                }
            }
            IDL_CON_record => {
                for _ in 0..leb128_decode(&mut tb) {
                    skip_leb128(&mut tb);
                    let it = sleb128_decode(&mut tb);
                    // This is just a quick check; we should be keeping
                    // track of all enclosing records to detect larger loops
                    if it == t {
                        idl_trap_with("skip_any: recursive record");
                    }
                    skip_any(buf, typtbl, it, depth + 1);
                }
            }
            IDL_CON_variant => {
                let n = leb128_decode(&mut tb);
                let i = leb128_decode(buf);
                if i >= n {
                    idl_trap_with("skip_any: variant tag too large");
                }
                for _ in 0..i {
                    skip_leb128(&mut tb);
                    skip_leb128(&mut tb);
                }
                skip_leb128(&mut tb);
                let it = sleb128_decode(&mut tb);
                skip_any(buf, typtbl, it, 0);
            }
            IDL_CON_func => {
                idl_trap_with("skip_any: func");
            }
            IDL_CON_service => {
                idl_trap_with("skip_any: service");
            }
            IDL_CON_alias => {
                // See Note [mutable stable values] in codegen/compile.ml
                let it = sleb128_decode(&mut tb);
                let tag = read_byte_tag(buf);
                if tag == 0 {
                    buf.advance(8);
                    // this is the contents (not a reference)
                    skip_any(buf, typtbl, it, 0);
                } else {
                    buf.advance(4);
                }
            }
            _ => {
                // Future type
                let n_data = leb128_decode(buf);
                let n_ref = leb128_decode(buf);
                buf.advance(n_data);
                if n_ref > 0 {
                    idl_trap_with("skip_any: skipping references");
                }
            }
        }
    }
}

/*
This finds a field in a record.

Preconditions:
  tb:     points into the type table,
          into the sequence of tags/types that are the argument of IDL_CON_record,
          at the tag
  b:      points into the data buffer, at value corresponding to the field
          pointed to by tb
  typtbl: the type table
  tag:    the desired tag
  n:      the number of fields left in the data

If the tag exists:
  return value: 1
  tb:    points at the type corresponding to the found field
  b:     points at the value corresponding to the found field
  n:     the number of fields left after the found field

If the tag does not exist:
  return value: 0
  tb:    points at the tag of the first field with a higher tag
         or at the end of the buffer
  b:     points at the value corresponding to that field
         or at the value past the record
  n:     the number of fields left, including the field pointed to by tb
*/
#[no_mangle]
unsafe extern "C" fn find_field(
    tb: *mut Buf,
    buf: *mut Buf,
    typtbl: *mut *mut u8,
    tag: u32,
    n: *mut u8,
) -> u32 {
    while *n > 0 {
        let last_p = (*tb).ptr;
        let this_tag = leb128_decode(tb);
        if this_tag < tag {
            let it = sleb128_decode(tb);
            skip_any(buf, typtbl, it, 0);
            *n -= 1;
        } else if tag == this_tag {
            *n -= 1;
            return 1;
        } else {
            // Rewind reading tag
            (*tb).ptr = last_p;
            return 0;
        }
    }

    0
}

#[no_mangle]
unsafe extern "C" fn skip_fields(tb: *mut Buf, buf: *mut Buf, typtbl: *mut *mut u8, n: *mut u8) {
    while *n > 0 {
        skip_leb128(tb);
        let it = sleb128_decode(tb);
        skip_any(buf, typtbl, it, 0);
        *n -= 1;
    }
}