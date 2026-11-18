//! Mirrors System.Text.Json formatting used by Services/ClipMetadata.cs and ViewModels/SettingsViewModel.cs.

use serde::Serialize;
use std::io;

/// serde_json formatter reproducing System.Text.Json's indented output + `JavaScriptEncoder.Default`.
#[derive(Default)]
pub(crate) struct NetFormatter {
    indent: usize,
    has_value: bool,
}

impl NetFormatter {
    fn newline_indent<W: ?Sized + io::Write>(&self, w: &mut W) -> io::Result<()> {
        w.write_all(b"\r\n")?;
        for _ in 0..self.indent {
            w.write_all(b"  ")?;
        }
        Ok(())
    }
}

impl serde_json::ser::Formatter for NetFormatter {
    fn begin_array<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.indent += 1;
        self.has_value = false;
        w.write_all(b"[")
    }
    fn end_array<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.indent -= 1;
        if self.has_value {
            self.newline_indent(w)?;
        }
        w.write_all(b"]")
    }
    fn begin_array_value<W: ?Sized + io::Write>(&mut self, w: &mut W, first: bool) -> io::Result<()> {
        if !first {
            w.write_all(b",")?;
        }
        self.newline_indent(w)
    }
    fn end_array_value<W: ?Sized + io::Write>(&mut self, _w: &mut W) -> io::Result<()> {
        self.has_value = true;
        Ok(())
    }
    fn begin_object<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.indent += 1;
        self.has_value = false;
        w.write_all(b"{")
    }
    fn end_object<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.indent -= 1;
        if self.has_value {
            self.newline_indent(w)?;
        }
        w.write_all(b"}")
    }
    fn begin_object_key<W: ?Sized + io::Write>(&mut self, w: &mut W, first: bool) -> io::Result<()> {
        if !first {
            w.write_all(b",")?;
        }
        self.newline_indent(w)
    }
    fn begin_object_value<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        w.write_all(b": ")
    }
    fn end_object_value<W: ?Sized + io::Write>(&mut self, _w: &mut W) -> io::Result<()> {
        self.has_value = true;
        Ok(())
    }
    fn write_f64<W: ?Sized + io::Write>(&mut self, w: &mut W, v: f64) -> io::Result<()> {
        // .NET shortest-round-trip G formatting: scientific below 1e-4 or at 1e17.
        // Use Rust's shortest digits, but .NET's exponent spelling and notation thresholds.
        if v != 0.0 && (v.abs() < 1e-4 || v.abs() >= 1e17) {
            write_exponential(w, &format!("{v:e}"))
        } else {
            write!(w, "{v}")
        }
    }
    fn write_f32<W: ?Sized + io::Write>(&mut self, w: &mut W, v: f32) -> io::Result<()> {
        if v != 0.0 && (v.abs() < 1e-4 || v.abs() >= 1e9) {
            write_exponential(w, &format!("{v:e}"))
        } else {
            write!(w, "{v}")
        }
    }
    fn write_string_fragment<W: ?Sized + io::Write>(&mut self, w: &mut W, fragment: &str) -> io::Result<()> {
        // JavaScriptEncoder.Default: only printable ASCII passes, minus HTML-sensitive characters.
        for ch in fragment.chars() {
            let c = ch as u32;
            let plain = (0x20..0x7F).contains(&c) && !matches!(ch, '<' | '>' | '&' | '\'' | '+' | '`');
            if plain {
                w.write_all(&[c as u8])?;
            } else {
                let mut buf = [0u16; 2];
                for unit in ch.encode_utf16(&mut buf) {
                    write!(w, "\\u{:04X}", unit)?;
                }
            }
        }
        Ok(())
    }
    fn write_char_escape<W: ?Sized + io::Write>(
        &mut self,
        w: &mut W,
        escape: serde_json::ser::CharEscape,
    ) -> io::Result<()> {
        use serde_json::ser::CharEscape::*;
        // System.Text.Json writes `"` as \u0022 and `\` as \\; control chars \n \r \t \b \f as short escapes.
        let s: &[u8] = match escape {
            Quote => b"\\u0022",
            ReverseSolidus => b"\\\\",
            Solidus => b"/",
            Backspace => b"\\b",
            FormFeed => b"\\f",
            LineFeed => b"\\n",
            CarriageReturn => b"\\r",
            Tab => b"\\t",
            AsciiControl(b) => return write!(w, "\\u{:04X}", b),
        };
        w.write_all(s)
    }
}

fn write_exponential<W: ?Sized + io::Write>(w: &mut W, value: &str) -> io::Result<()> {
    let (digits, exponent) = value.split_once('e').expect("LowerExp always has exponent");
    let exponent: i32 = exponent.parse().expect("LowerExp exponent is an integer");
    write!(w, "{digits}E{exponent:+03}")
}

pub(crate) fn to_string<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<String> {
    let mut out = Vec::new();
    Checked(value).serialize(&mut serde_json::Serializer::with_formatter(&mut out, NetFormatter::default()))?;
    // All string fragments are ASCII escaped, and JSON punctuation is ASCII.
    Ok(String::from_utf8(out).expect("JSON formatter emits UTF-8"))
}

// serde_json normally silently converts non-finite floats to null. System.Text.Json rejects
// them instead. Interpose once, including at every compound value, without serializing twice
// or buffering a Value (which would lose property order).
struct Checked<'a, T: ?Sized>(&'a T);
impl<T: Serialize + ?Sized> Serialize for Checked<'_, T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(FiniteSerializer(serializer))
    }
}
struct FiniteSerializer<S>(S);
struct Compound<C>(C);
macro_rules! scalar {
    ($($method:ident($($arg:ident: $ty:ty),*);)*) => {$(
        fn $method(self, $($arg: $ty),*) -> Result<Self::Ok, Self::Error> {
            self.0.$method($($arg),*)
        }
    )*};
}
macro_rules! compound {
    ($($method:ident($($arg:ident: $ty:ty),*) -> $associated:ident;)*) => {$(
        fn $method(self, $($arg: $ty),*) -> Result<Self::$associated, Self::Error> {
            self.0.$method($($arg),*).map(Compound)
        }
    )*};
}
impl<S: serde::Serializer> serde::Serializer for FiniteSerializer<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    type SerializeSeq = Compound<S::SerializeSeq>;
    type SerializeTuple = Compound<S::SerializeTuple>;
    type SerializeTupleStruct = Compound<S::SerializeTupleStruct>;
    type SerializeTupleVariant = Compound<S::SerializeTupleVariant>;
    type SerializeMap = Compound<S::SerializeMap>;
    type SerializeStruct = Compound<S::SerializeStruct>;
    type SerializeStructVariant = Compound<S::SerializeStructVariant>;
    scalar! {
        serialize_bool(v: bool); serialize_i8(v: i8); serialize_i16(v: i16);
        serialize_i32(v: i32); serialize_i64(v: i64); serialize_i128(v: i128);
        serialize_u8(v: u8); serialize_u16(v: u16); serialize_u32(v: u32);
        serialize_u64(v: u64); serialize_u128(v: u128); serialize_char(v: char);
        serialize_str(v: &str); serialize_bytes(v: &[u8]); serialize_none();
        serialize_unit(); serialize_unit_struct(name: &'static str);
        serialize_unit_variant(name: &'static str, index: u32, variant: &'static str);
    }
    fn serialize_f32(self, v: f32) -> Result<Self::Ok, Self::Error> {
        if v.is_finite() {
            self.0.serialize_f32(v)
        } else {
            Err(serde::ser::Error::custom("System.Text.Json cannot write a non-finite float"))
        }
    }
    fn serialize_f64(self, v: f64) -> Result<Self::Ok, Self::Error> {
        if v.is_finite() {
            self.0.serialize_f64(v)
        } else {
            Err(serde::ser::Error::custom("System.Text.Json cannot write a non-finite float"))
        }
    }
    fn serialize_some<T: Serialize + ?Sized>(self, v: &T) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_some(&Checked(v))
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        v: &T,
    ) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_newtype_struct(name, &Checked(v))
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        v: &T,
    ) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_newtype_variant(name, index, variant, &Checked(v))
    }
    compound! {
        serialize_seq(len: Option<usize>) -> SerializeSeq;
        serialize_tuple(len: usize) -> SerializeTuple;
        serialize_tuple_struct(name: &'static str,len: usize) -> SerializeTupleStruct;
        serialize_tuple_variant(name: &'static str,index: u32,variant: &'static str,len: usize) -> SerializeTupleVariant;
        serialize_map(len: Option<usize>) -> SerializeMap;
        serialize_struct(name: &'static str,len: usize) -> SerializeStruct;
        serialize_struct_variant(name: &'static str,index: u32,variant: &'static str,len: usize) -> SerializeStructVariant;
    }
    fn collect_str<T: std::fmt::Display + ?Sized>(self, v: &T) -> Result<Self::Ok, Self::Error> {
        self.0.collect_str(v)
    }
    fn is_human_readable(&self) -> bool {
        self.0.is_human_readable()
    }
}
macro_rules! sequential {
    ($($trait:ident::$method:ident;)*) => {$(
        impl<C: serde::ser::$trait> serde::ser::$trait for Compound<C> {
            type Ok = C::Ok; type Error = C::Error;
            fn $method<T: Serialize + ?Sized>(&mut self,v: &T) -> Result<(),Self::Error> { self.0.$method(&Checked(v)) }
            fn end(self) -> Result<Self::Ok,Self::Error> { self.0.end() }
        }
    )*};
}
sequential! {
    SerializeSeq::serialize_element; SerializeTuple::serialize_element;
    SerializeTupleStruct::serialize_field; SerializeTupleVariant::serialize_field;
}
macro_rules! structures {
    ($($trait:ident;)*) => {$(
        impl<C: serde::ser::$trait> serde::ser::$trait for Compound<C> {
            type Ok = C::Ok; type Error = C::Error;
            fn serialize_field<T: Serialize + ?Sized>(&mut self,key: &'static str,v: &T) -> Result<(),Self::Error> { self.0.serialize_field(key,&Checked(v)) }
            fn skip_field(&mut self,key: &'static str) -> Result<(),Self::Error> { self.0.skip_field(key) }
            fn end(self) -> Result<Self::Ok,Self::Error> { self.0.end() }
        }
    )*};
}
structures! { SerializeStruct; SerializeStructVariant; }
impl<C: serde::ser::SerializeMap> serde::ser::SerializeMap for Compound<C> {
    type Ok = C::Ok;
    type Error = C::Error;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), Self::Error> {
        self.0.serialize_key(&Checked(v))
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), Self::Error> {
        self.0.serialize_value(&Checked(v))
    }
    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nonfinite_numbers_are_rejected_including_nested_values() {
        assert!(to_string(&f64::NAN).is_err());
        assert!(to_string(&vec![Some(f64::INFINITY)]).is_err());
        assert!(to_string(&std::collections::BTreeMap::from([("bad", f32::NEG_INFINITY)])).is_err());
        assert_eq!(to_string(&Option::<f64>::None).unwrap(), "null");
    }
    #[test]
    fn dotnet8_float_goldens() {
        for (v, s) in [
            (0.0, "0"),
            (-0.0, "-0"),
            (0.0001, "0.0001"),
            (1e-5, "1E-05"),
            (1e16, "10000000000000000"),
            (1e17, "1E+17"),
            (1e20, "1E+20"),
            (f64::from_bits(1), "5E-324"),
            (1.2345678901234567e30, "1.2345678901234567E+30"),
        ] {
            assert_eq!(to_string(&v).unwrap(), s);
        }
        for (v, s) in [
            (0f32, "0"),
            (-0f32, "-0"),
            (1e-4, "0.0001"),
            (1e-5, "1E-05"),
            (1e8, "100000000"),
            (1e9, "1E+09"),
            (f32::from_bits(1), "1E-45"),
        ] {
            assert_eq!(to_string(&v).unwrap(), s);
        }
    }
    #[test]
    fn arbitrary_nested_values_and_escaping() {
        #[derive(Serialize)]
        struct Data<'a> {
            strings: Vec<&'a str>,
            nested: Vec<Vec<i32>>,
        }
        let d = Data { strings: vec!["é😀\"\\\n<>&'+`\u{7f}"], nested: vec![vec![], vec![1]] };
        assert_eq!(to_string(&d).unwrap(),"{\r\n  \"strings\": [\r\n    \"\\u00E9\\uD83D\\uDE00\\u0022\\\\\\n\\u003C\\u003E\\u0026\\u0027\\u002B\\u0060\\u007F\"\r\n  ],\r\n  \"nested\": [\r\n    [],\r\n    [\r\n      1\r\n    ]\r\n  ]\r\n}");
    }
}
