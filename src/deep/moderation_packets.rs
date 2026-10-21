// Alduin moderation packets (custom family FAMILY_MODERATION, same hand-written style as the other
// deep packets): official warnings shown to the player, their acknowledgement, and in-game reports.
use eolib::data::{EoReader, EoReaderError, EoSerialize, EoSerializeError, EoWriter};

/// Moderation_Open (server → client): show an official warning until acknowledged.
#[derive(Debug, Default, PartialEq, Eq, Clone)]
pub struct WarningOpenServerPacket {
    pub warning_id: i32,
    pub category: String,
    pub reason: String,
    pub issued_at: String,
}

impl EoSerialize for WarningOpenServerPacket {
    fn deserialize(reader: &EoReader) -> Result<Self, EoReaderError> {
        let mode = reader.get_chunked_reading_mode();
        reader.set_chunked_reading_mode(true);
        let mut packet = Self::default();
        packet.warning_id = reader.get_int();
        reader.next_chunk()?;
        packet.category = reader.get_string();
        reader.next_chunk()?;
        packet.reason = reader.get_string();
        reader.next_chunk()?;
        packet.issued_at = reader.get_string();
        reader.set_chunked_reading_mode(mode);
        Ok(packet)
    }

    fn serialize(&self, writer: &mut EoWriter) -> Result<(), EoSerializeError> {
        writer.add_int(self.warning_id)?;
        writer.add_byte(0xff);
        writer.add_string(&self.category);
        writer.add_byte(0xff);
        writer.add_string(&self.reason);
        writer.add_byte(0xff);
        writer.add_string(&self.issued_at);
        Ok(())
    }
}

/// Moderation_Accept (client → server): the player acknowledged a warning.
#[derive(Debug, Default, PartialEq, Eq, Clone)]
pub struct WarningAcceptClientPacket {
    pub warning_id: i32,
}

impl EoSerialize for WarningAcceptClientPacket {
    fn deserialize(reader: &EoReader) -> Result<Self, EoReaderError> {
        Ok(Self {
            warning_id: reader.get_int(),
        })
    }

    fn serialize(&self, writer: &mut EoWriter) -> Result<(), EoSerializeError> {
        writer.add_int(self.warning_id)?;
        Ok(())
    }
}

/// Moderation_Report (client → server): report the player with this session id.
#[derive(Debug, Default, PartialEq, Eq, Clone)]
pub struct ReportPlayerClientPacket {
    pub player_id: i32,
    pub category: String,
    pub description: String,
}

impl EoSerialize for ReportPlayerClientPacket {
    fn deserialize(reader: &EoReader) -> Result<Self, EoReaderError> {
        let mode = reader.get_chunked_reading_mode();
        reader.set_chunked_reading_mode(true);
        let mut packet = Self::default();
        packet.player_id = reader.get_short();
        reader.next_chunk()?;
        packet.category = reader.get_string();
        reader.next_chunk()?;
        packet.description = reader.get_string();
        reader.set_chunked_reading_mode(mode);
        Ok(packet)
    }

    fn serialize(&self, writer: &mut EoWriter) -> Result<(), EoSerializeError> {
        writer.add_short(self.player_id)?;
        writer.add_byte(0xff);
        writer.add_string(&self.category);
        writer.add_byte(0xff);
        writer.add_string(&self.description);
        Ok(())
    }
}

/// Moderation_Reply (server → client): outcome of an in-game report.
/// code 0 = submitted (reference + tracking code given), 1 = invalid, 2 = too many reports, 3 = player not found.
#[derive(Debug, Default, PartialEq, Eq, Clone)]
pub struct ReportReplyServerPacket {
    pub code: i32,
    pub reference: String,
    pub tracking_code: String,
}

impl EoSerialize for ReportReplyServerPacket {
    fn deserialize(reader: &EoReader) -> Result<Self, EoReaderError> {
        let mode = reader.get_chunked_reading_mode();
        reader.set_chunked_reading_mode(true);
        let mut packet = Self::default();
        packet.code = reader.get_char();
        reader.next_chunk()?;
        packet.reference = reader.get_string();
        reader.next_chunk()?;
        packet.tracking_code = reader.get_string();
        reader.set_chunked_reading_mode(mode);
        Ok(packet)
    }

    fn serialize(&self, writer: &mut EoWriter) -> Result<(), EoSerializeError> {
        writer.add_char(self.code)?;
        writer.add_byte(0xff);
        writer.add_string(&self.reference);
        writer.add_byte(0xff);
        writer.add_string(&self.tracking_code);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    fn roundtrip<T: EoSerialize + PartialEq + std::fmt::Debug>(packet: T) {
        let mut writer = EoWriter::new();
        packet.serialize(&mut writer).unwrap();
        let reader = EoReader::new(Bytes::from(writer.to_byte_array().to_vec()));
        assert_eq!(T::deserialize(&reader).unwrap(), packet);
    }

    #[test]
    fn moderation_packets_roundtrip() {
        roundtrip(WarningOpenServerPacket {
            warning_id: 70000,
            category: "Harassment".into(),
            reason: "Repeated insults".into(),
            issued_at: "2026-10-20 21:00 UTC".into(),
        });
        roundtrip(WarningAcceptClientPacket { warning_id: 12 });
        roundtrip(ReportPlayerClientPacket {
            player_id: 345,
            category: "Scamming".into(),
            description: "Took my gold".into(),
        });
        roundtrip(ReportReplyServerPacket {
            code: 0,
            reference: "R-ABC123".into(),
            tracking_code: "K7Q2".into(),
        });
    }
}
