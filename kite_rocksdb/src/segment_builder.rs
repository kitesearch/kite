use std::collections::HashMap;

use kite::{Document, Term, TermRef};
use kite::schema::FieldRef;
use kite::segment::Segment;
use byteorder::{LittleEndian, WriteBytesExt};
use roaring::RoaringBitmap;
use fnv::FnvHashMap;

use key_builder::KeyBuilder;


#[derive(Debug)]
pub struct SegmentBuilder {
    current_doc: u16,
    pub term_dictionary: HashMap<Term, TermRef>,
    current_term_ref: u32,
    pub term_directories: FnvHashMap<(FieldRef, TermRef), RoaringBitmap>,
    pub statistics: FnvHashMap<Vec<u8>, i64>,
    pub stored_field_values: FnvHashMap<(FieldRef, u16, Vec<u8>), Vec<u8>>,
}


#[derive(Debug)]
pub enum DocumentInsertError {
    /// Segment couldn't hold any more docs
    SegmentFull,
}


impl SegmentBuilder {
    pub fn new() -> SegmentBuilder {
        SegmentBuilder {
            current_doc: 0,
            term_dictionary: HashMap::new(),
            current_term_ref: 0,
            term_directories: FnvHashMap::default(),
            statistics: FnvHashMap::default(),
            stored_field_values: FnvHashMap::default(),
        }
    }

    fn get_term_ref(&mut self, term: &Term) -> TermRef {
        if let Some(term_ref) = self.term_dictionary.get(term) {
            return *term_ref;
        }

        // Add the term to the dictionary
        let term_ref = TermRef::new(self.current_term_ref);
        self.current_term_ref += 1;
        self.term_dictionary.insert(term.clone(), term_ref);

        term_ref
    }

    pub fn add_document(&mut self, doc: &Document) -> Result<u16, DocumentInsertError> {
        // Get document ord
        let doc_id = self.current_doc;
        self.current_doc += 1;
        try!(self.current_doc.checked_add(1).ok_or(DocumentInsertError::SegmentFull));

        // Insert indexed fields
        let mut term_frequencies = FnvHashMap::default();
        for (field, tokens) in doc.indexed_fields.iter() {
            let mut field_token_count = 0;

            for (term, positions) in tokens.iter() {
                let frequency = positions.len();
                field_token_count += frequency;

                // Get term ref
                let term_ref = self.get_term_ref(term);

                // Term frequency
                let mut term_frequency = term_frequencies.entry(term_ref).or_insert(0);
                *term_frequency += frequency;

                // Write directory list
                self.term_directories.entry((*field, term_ref)).or_insert_with(RoaringBitmap::new).insert(doc_id as u32);

                // Write term frequency
                // 1 is by far the most common frequency. At search time, we interpret a missing
                // key as meaning there is a term frequency of 1
                if frequency != 1 {
                    let mut value_type = vec![b't', b'f'];
                    value_type.extend(term_ref.ord().to_string().as_bytes());

                    let mut frequency_bytes: Vec<u8> = Vec::new();
                    frequency_bytes.write_i64::<LittleEndian>(frequency as i64).unwrap();

                    self.stored_field_values.insert((*field, doc_id, value_type), frequency_bytes);
                }

                // Increment term document frequency
                let stat_name = KeyBuilder::segment_stat_term_doc_frequency_stat_name(field.ord(), term_ref.ord());
                let mut stat = self.statistics.entry(stat_name).or_insert(0);
                *stat += 1;
            }

            // Field length
            // Used by the BM25 similarity model
            let length = ((field_token_count as f32).sqrt() - 1.0) * 3.0;
            let length = if length > 255.0 { 255.0 } else { length } as u8;
            if length != 0 {
                self.stored_field_values.insert((*field, doc_id, b"len".to_vec()), vec![length]);
            }

            // Increment total field docs
            {
                let stat_name = KeyBuilder::segment_stat_total_field_docs_stat_name(field.ord());
                let mut stat = self.statistics.entry(stat_name).or_insert(0);
                *stat += 1;
            }

            // Increment total field tokens
            {
                let stat_name = KeyBuilder::segment_stat_total_field_tokens_stat_name(field.ord());
                let mut stat = self.statistics.entry(stat_name).or_insert(0);
                *stat += field_token_count as i64;
            }
        }

        // Insert stored fields
        for (field, value) in doc.stored_fields.iter() {
            self.stored_field_values.insert((*field, doc_id, b"val".to_vec()), value.to_bytes());
        }

        // Increment total docs
        {
            let mut stat = self.statistics.entry(b"total_docs".to_vec()).or_insert(0);
            *stat += 1;
        }

        Ok(doc_id)
    }
}


impl Segment for SegmentBuilder {
    fn id(&self) -> u32 {
        0
    }

    fn load_statistic(&self, stat_name: &[u8]) -> Result<Option<i64>, String> {
        Ok(self.statistics.get(stat_name).cloned())
    }

    fn load_stored_field_value_raw(&self, doc_ord: u16, field_ref: FieldRef, value_type: &[u8]) -> Result<Option<Vec<u8>>, String> {
        Ok(self.stored_field_values.get(&(field_ref, doc_ord, value_type.to_vec())).cloned())
    }

    fn load_term_directory(&self, field_ref: FieldRef, term_ref: TermRef) -> Result<Option<RoaringBitmap>, String> {
        Ok(self.term_directories.get(&(field_ref, term_ref)).cloned())
    }

    fn load_deletion_list(&self) -> Result<Option<RoaringBitmap>, String> {
        Ok(None)
    }
}
