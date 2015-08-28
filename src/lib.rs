extern crate byteorder;
extern crate itertools;
extern crate rustc_serialize;
extern crate xml as xml_rs;

pub mod binary;
pub mod xml;

use byteorder::Error as ByteorderError;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::io::Error as IoError;
use std::string::FromUtf16Error;

#[derive(Clone, Debug, PartialEq)]
pub enum Plist {
	Array(Vec<Plist>),
	Dictionary(HashMap<String, Plist>),
	Boolean(bool),
	Data(Vec<u8>),
	Date(String),
	Real(f64),
	Integer(i64),
	String(String)
}

#[derive(Debug, PartialEq)]
pub enum PlistEvent {
	StartPlist,
	EndPlist,

	StartArray(Option<u64>),
	EndArray,

	StartDictionary(Option<u64>),
	EndDictionary,

	BooleanValue(bool),
	DataValue(Vec<u8>),
	DateValue(String),
	IntegerValue(i64),
	RealValue(f64),
	StringValue(String),
}

pub type ParserResult<T> = Result<T, ParserError>;

#[derive(Debug)]
pub enum ParserError {
	InvalidData,
	UnexpectedEof,
	UnsupportedType,
	Io(IoError)
}

impl From<IoError> for ParserError {
	fn from(io_error: IoError) -> ParserError {
		ParserError::Io(io_error)
	}
}

impl From<ByteorderError> for ParserError {
	fn from(err: ByteorderError) -> ParserError {
		match err {
			ByteorderError::UnexpectedEOF => ParserError::UnexpectedEof,
			ByteorderError::Io(err) => ParserError::Io(err)
		}
	}
}

impl From<FromUtf16Error> for ParserError {
	fn from(_: FromUtf16Error) -> ParserError {
		ParserError::InvalidData
	}
}

pub enum StreamingParser<R: Read+Seek> {
	Xml(xml::StreamingParser<R>),
	Binary(binary::StreamingParser<R>)
}

impl<R: Read+Seek> StreamingParser<R> {
	pub fn new(mut reader: R) -> StreamingParser<R> {
		match StreamingParser::is_binary(&mut reader) {
			Ok(true) => StreamingParser::Binary(binary::StreamingParser::new(reader)),
			Ok(false) | Err(_) => StreamingParser::Xml(xml::StreamingParser::new(reader))
		}
	}

	fn is_binary(reader: &mut R) -> Result<bool, IoError> {
		try!(reader.seek(SeekFrom::Start(0)));
		let mut magic = [0; 8];
		try!(reader.read(&mut magic));

		Ok(if &magic == b"bplist00" {
			true
		} else {
			false
		})
	}
}

impl<R: Read+Seek> Iterator for StreamingParser<R> {
	type Item = ParserResult<PlistEvent>;

	fn next(&mut self) -> Option<ParserResult<PlistEvent>> {
		match *self {
			StreamingParser::Xml(ref mut parser) => parser.next(),
			StreamingParser::Binary(ref mut parser) => parser.next()
		}
	}
}

pub type BuilderResult<T> = Result<T, BuilderError>;

#[derive(Debug)]
pub enum BuilderError {
	InvalidEvent,
	UnsupportedDictionaryKey,
	ParserError(ParserError)
}

impl From<ParserError> for BuilderError {
	fn from(err: ParserError) -> BuilderError {
		BuilderError::ParserError(err)
	}
}

pub struct Builder<T> {
	stream: T,
	token: Option<PlistEvent>,
}

impl<R: Read + Seek> Builder<StreamingParser<R>> {
	pub fn new(reader: R) -> Builder<StreamingParser<R>> {
		Builder::from_event_stream(StreamingParser::new(reader))
	}
}

impl<T:Iterator<Item=ParserResult<PlistEvent>>> Builder<T> {
	pub fn from_event_stream(stream: T) -> Builder<T> {
		Builder {
			stream: stream,
			token: None
		}
	}

	pub fn build(mut self) -> BuilderResult<Plist> {
		try!(self.bump());
		if let Some(PlistEvent::StartPlist) = self.token {
			try!(self.bump());
		}

		let plist = try!(self.build_value());
		try!(self.bump());
		match self.token {
			None => (),
			Some(PlistEvent::EndPlist) => try!(self.bump()),
			// The stream should have finished
			_ => return Err(BuilderError::InvalidEvent)
		};
		Ok(plist)
	}

	fn bump(&mut self) -> BuilderResult<()> {
		self.token = match self.stream.next() {
			Some(Ok(token)) => Some(token),
			Some(Err(err)) => return Err(BuilderError::ParserError(err)),
			None => None,
		};
		Ok(())
	}

	fn build_value(&mut self) -> BuilderResult<Plist> {
		match self.token.take() {
			Some(PlistEvent::StartPlist) => Err(BuilderError::InvalidEvent),
			Some(PlistEvent::EndPlist) => Err(BuilderError::InvalidEvent),

			Some(PlistEvent::StartArray(len)) => Ok(Plist::Array(try!(self.build_array(len)))),
			Some(PlistEvent::StartDictionary(len)) => Ok(Plist::Dictionary(try!(self.build_dict(len)))),

			Some(PlistEvent::BooleanValue(b)) => Ok(Plist::Boolean(b)),
			Some(PlistEvent::DataValue(d)) => Ok(Plist::Data(d)),
			Some(PlistEvent::DateValue(d)) => Ok(Plist::Date(d)),
			Some(PlistEvent::IntegerValue(i)) => Ok(Plist::Integer(i)),
			Some(PlistEvent::RealValue(f)) => Ok(Plist::Real(f)),
			Some(PlistEvent::StringValue(s)) => Ok(Plist::String(s)),

			Some(PlistEvent::EndArray) => Err(BuilderError::InvalidEvent),
			Some(PlistEvent::EndDictionary) => Err(BuilderError::InvalidEvent),

			// The stream should not have ended here
			None => Err(BuilderError::InvalidEvent)
		}
	}

	fn build_array(&mut self, len: Option<u64>) -> Result<Vec<Plist>, BuilderError> {	
		let mut values = match len {
			Some(len) => Vec::with_capacity(len as usize),
			None => Vec::new()
		};

		loop {
			try!(self.bump());
			if let Some(PlistEvent::EndArray) = self.token {
				self.token.take();
				return Ok(values);
			}
			values.push(try!(self.build_value()));
		}
	}

	fn build_dict(&mut self, len: Option<u64>) -> Result<HashMap<String, Plist>, BuilderError> {
		let mut values = match len {
			Some(len) => HashMap::with_capacity(len as usize),
			None => HashMap::new()
		};

		loop {
			try!(self.bump());
			match self.token.take() {
				Some(PlistEvent::EndDictionary) => return Ok(values),
				Some(PlistEvent::StringValue(s)) => {
					try!(self.bump());
					values.insert(s, try!(self.build_value()));
				},
				_ => {
					// Only string keys are supported in plists
					return Err(BuilderError::UnsupportedDictionaryKey)
				}
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use std::collections::HashMap;

	use super::*;

	#[test]
	fn builder() {
		use super::PlistEvent::*;

		// Input

		let events = vec![
			StartPlist,
			StartDictionary(None),
			StringValue("Author".to_owned()),
			StringValue("William Shakespeare".to_owned()),
			StringValue("Lines".to_owned()),
			StartArray(None),
			StringValue("It is a tale told by an idiot,".to_owned()),
			StringValue("Full of sound and fury, signifying nothing.".to_owned()),
			EndArray,
			StringValue("Birthdate".to_owned()),
			IntegerValue(1564),
			StringValue("Height".to_owned()),
			RealValue(1.60),
			EndDictionary,
			EndPlist,
		];

		let builder = Builder::from_event_stream(events.into_iter().map(|e| Ok(e)));
		let plist = builder.build();

		// Expected output

		let mut lines = Vec::new();
		lines.push(Plist::String("It is a tale told by an idiot,".to_owned()));
		lines.push(Plist::String("Full of sound and fury, signifying nothing.".to_owned()));

		let mut dict = HashMap::new();
		dict.insert("Author".to_owned(), Plist::String("William Shakespeare".to_owned()));
		dict.insert("Lines".to_owned(), Plist::Array(lines));
		dict.insert("Birthdate".to_owned(), Plist::Integer(1564));
		dict.insert("Height".to_owned(), Plist::Real(1.60));

		assert_eq!(plist.unwrap(), Plist::Dictionary(dict));
	}
}