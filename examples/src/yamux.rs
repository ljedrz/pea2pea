//! A simple implementation of the Yamux multiplexer, scoped to what the `libp2p`
//! example needs (it only ever runs beneath the noise codec, whole frames at a time).

use std::{fmt, io};

use bytes::{Buf, BufMut, Bytes, BytesMut};
use pea2pea::ConnectionSide;
use tokio_util::codec::{Decoder, Encoder};

// the version used in Yamux message headers
pub const VERSION: u8 = 0;

// the numeric ID of a Yamux stream
pub type StreamId = u32;

// a header describing a Yamux message
#[derive(Clone, PartialEq, Eq)]
pub struct Header {
    version: u8,
    pub ty: Ty,
    pub flags: Vec<Flag>,
    pub stream_id: StreamId,
    pub length: u32,
}

impl fmt::Debug for Header {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // note: the version and length are hidden for brevity
        write!(
            f,
            "{{ ID: {}, {}, {:?} }}",
            self.stream_id, self.ty, self.flags,
        )
    }
}

// a full Yamux message
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub header: Header,
    pub payload: Bytes,
}

impl Frame {
    // creates a new data message for the given Yamux stream ID
    pub fn data(stream_id: u32, flags: Vec<Flag>, data: Option<Bytes>) -> Self {
        let payload = data.unwrap_or_default();

        let header = Header {
            version: VERSION,
            ty: Ty::Data,
            flags,
            stream_id,
            length: payload.len() as u32,
        };

        Self { header, payload }
    }

    // creates a session-level ping message; the opaque value is carried in the
    // length field, and there is no payload
    pub fn ping(flags: Vec<Flag>, opaque: u32) -> Self {
        let header = Header {
            version: VERSION,
            ty: Ty::Ping,
            flags,
            stream_id: 0,
            length: opaque,
        };

        Self {
            header,
            payload: Default::default(),
        }
    }

    pub fn terminate(stream_id: u32) -> Self {
        let header = Header {
            version: VERSION,
            ty: Ty::GoAway,
            flags: vec![],
            stream_id,
            length: Termination::Normal as u32,
        };

        Self {
            header,
            payload: Default::default(),
        }
    }
}

// a codec used to (de/en)code Yamux frames
pub struct Codec {
    // client or server; kept for documentation purposes (the client side
    // should use odd stream IDs, the server side even ones)
    #[allow(dead_code)]
    mode: Side,
}

impl Codec {
    pub fn new(conn_side: ConnectionSide) -> Self {
        let mode = if conn_side == ConnectionSide::Initiator {
            Side::Client
        } else {
            Side::Server
        };

        Self { mode }
    }
}

// indicates the type of a Yamux message
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Ty {
    // used to transmit data
    Data = 0x0,
    // used to update the sender's receive window size
    WindowUpdate = 0x1,
    // used to measure RTT
    Ping = 0x2,
    // used to close a session
    GoAway = 0x3,
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Data => write!(f, "Data"),
            Self::WindowUpdate => write!(f, "Window Update"),
            Self::Ping => write!(f, "Ping"),
            Self::GoAway => write!(f, "Go Away"),
        }
    }
}

impl TryFrom<u8> for Ty {
    type Error = io::Error;

    fn try_from(ty: u8) -> io::Result<Self> {
        match ty {
            0x0 => Ok(Self::Data),
            0x1 => Ok(Self::WindowUpdate),
            0x2 => Ok(Self::Ping),
            0x3 => Ok(Self::GoAway),
            _ => Err(io::ErrorKind::InvalidData.into()),
        }
    }
}

// indicates the termination of a session
#[repr(u32)]
pub enum Termination {
    Normal = 0,
    #[allow(dead_code)]
    ProtocolError = 1,
    #[allow(dead_code)]
    InternalError = 2,
}

// additional information related to the Yamux message type
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Flag {
    // signals the start of a new stream
    Syn = 0x1,
    // acknowledges the start of a new stream
    Ack = 0x2,
    // performs a half-close of a stream
    Fin = 0x4,
    // resets a stream immediately
    Rst = 0x8,
}

impl fmt::Display for Flag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syn => write!(f, "SYN"),
            Self::Ack => write!(f, "ACK"),
            Self::Fin => write!(f, "FIN"),
            Self::Rst => write!(f, "RST"),
        }
    }
}

impl fmt::Debug for Flag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

impl TryFrom<u8> for Flag {
    type Error = io::Error;

    fn try_from(flag: u8) -> io::Result<Self> {
        match flag {
            0x1 => Ok(Self::Syn),
            0x2 => Ok(Self::Ack),
            0x4 => Ok(Self::Fin),
            0x8 => Ok(Self::Rst),
            _ => Err(io::ErrorKind::InvalidData.into()),
        }
    }
}

// interpret the flags encoded in a Yamux message
fn decode_flags(flags: u16) -> io::Result<Vec<Flag>> {
    if flags & !0xf != 0 {
        return Err(io::ErrorKind::InvalidData.into());
    }

    Ok([Flag::Syn, Flag::Ack, Flag::Fin, Flag::Rst]
        .into_iter()
        .filter(|flag| flags & (*flag as u16) != 0)
        .collect())
}

// encode the given flags in a Yamux message
fn encode_flags(flags: &[Flag]) -> u16 {
    let mut ret = 0u16;

    for flag in flags {
        ret |= *flag as u16;
    }

    ret
}

// the side of a Yamux connection
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    // client side should use odd stream IDs
    Client,
    // server side should use even stream IDs
    Server,
}

impl Decoder for Codec {
    type Item = Frame;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        // parse the Yamux header
        let version = src.get_u8();
        let ty = Ty::try_from(src.get_u8())?;
        let flags = decode_flags(src.get_u16())?;
        let stream_id = src.get_u32();
        let length = src.get_u32();

        let payload = match ty {
            // zero-copy: everything left in the buffer is the payload
            Ty::Data => src.split().freeze(),
            Ty::Ping => Bytes::copy_from_slice(&length.to_be_bytes()),
            // header-only frames; the length field carries their semantic value
            Ty::WindowUpdate | Ty::GoAway => Bytes::new(),
        };

        Ok(Some(Frame {
            header: Header {
                version,
                ty,
                flags,
                stream_id,
                length,
            },
            payload,
        }))
    }
}

impl Encoder<Frame> for Codec {
    type Error = io::Error;

    fn encode(&mut self, msg: Frame, dst: &mut BytesMut) -> Result<(), Self::Error> {
        // note: no need to use the underlying length-delimited codec for encoding

        // version
        dst.put_u8(msg.header.version);
        // type
        dst.put_u8(msg.header.ty as u8);
        // flags
        dst.put_u16(encode_flags(&msg.header.flags));
        // stream ID
        dst.put_u32(msg.header.stream_id);
        // length; for Data frames it is the payload size, for the header-only
        // types it carries their semantic value (ping opaque, GoAway code)
        dst.put_u32(msg.header.length);

        // data
        dst.put(msg.payload);

        Ok(())
    }
}
