// Copyright (C) 2021 Quickwit, Inc.
//
// Quickwit is offered under the AGPL v3.0 and as commercial software.
// For commercial licensing, contact us at hello@quickwit.io.
//
// AGPL:
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as
// published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program. If not, see <http://www.gnu.org/licenses/>.

use std::convert::TryInto;
use std::str::FromStr;

use chrono::NaiveDateTime;
use diesel::sql_types::{Nullable, Text};
use tracing::error;

use crate::postgresql::schema::{indexes, splits};
use crate::{
    IndexMetadata, MetastoreError, MetastoreResult, Split as QuickwitSplit, SplitMetadata,
    SplitState,
};

// A raw query that helps figure out if index exist, non-existant
// splits and not deletable splits.
pub const SELECT_SPLITS_FOR_INDEX: &str = r#"
SELECT i.index_id, s.split_id 
FROM indexes AS i 
LEFT JOIN (
    SELECT index_id, split_id 
    FROM splits
    WHERE split_id = ANY ($1)
) AS s 
ON i.index_id = s.index_id
WHERE i.index_id = $2"#;

#[derive(Queryable, QueryableByName, Debug, Clone)]
pub struct IndexIdSplitIdRow {
    #[sql_type = "Text"]
    pub index_id: String,
    #[sql_type = "Nullable<Text>"]
    pub split_id: Option<String>,
}

/// A model structure for handling index metadata in a database.
#[derive(Identifiable, Insertable, Queryable, Debug)]
#[primary_key(index_id)]
#[table_name = "indexes"]
pub struct Index {
    /// Index ID. The index ID identifies the index when querying the metastore.
    pub index_id: String,
    // A JSON string containing all of the IndexMetadata.
    pub index_metadata_json: String,
    /// Timestamp for tracking when the split was created.
    pub create_timestamp: NaiveDateTime,
    /// Timestamp for tracking when the split was last updated.
    pub update_timestamp: NaiveDateTime,
}

impl Index {
    /// Deserializes index metadata from JSON string stored in column and sets appropriate
    /// timestamps.
    pub fn index_metadata(&self) -> MetastoreResult<IndexMetadata> {
        let mut index_metadata = serde_json::from_str::<IndexMetadata>(&self.index_metadata_json)
            .map_err(|err| MetastoreError::InternalError {
            message: "Failed to deserialize index metadata.".to_string(),
            cause: anyhow::anyhow!(err),
        })?;
        // `create_timestamp` and `update_timestamp` are stored in dedicated columns but are also
        // duplicated in [`IndexMetadata`]. We must override the duplicates with the authentic
        // values upon deserialization.
        index_metadata.create_timestamp = self.create_timestamp.timestamp();
        index_metadata.update_timestamp = self.update_timestamp.timestamp();
        Ok(index_metadata)
    }
}

/// A model structure for handling split metadata in a database.
#[derive(Identifiable, Insertable, Associations, Queryable, Debug)]
#[belongs_to(Index)]
#[primary_key(split_id)]
#[table_name = "splits"]
pub struct Split {
    /// Split ID.
    pub split_id: String,
    /// The state of the split. With `update_timestamp`, this is the only mutable attribute of the
    /// split.
    pub split_state: String,
    /// If a timestamp field is available, the min timestamp of the split.
    pub time_range_start: Option<i64>,
    /// If a timestamp field is available, the max timestamp of the split.
    pub time_range_end: Option<i64>,
    /// Timestamp for tracking when the split was created.
    pub create_timestamp: NaiveDateTime,
    /// Timestamp for tracking when the split was last updated.
    pub update_timestamp: NaiveDateTime,
    /// A list of tags for categorizing and searching group of splits.
    pub tags: Vec<String>,
    // The split's metadata serialized as a JSON string.
    pub split_metadata_json: String,
    /// Index ID. It is used as a foreign key in the database.
    pub index_id: String,
}

impl Split {
    /// Deserializes and returns the split's metadata.
    fn split_metadata(&self) -> MetastoreResult<SplitMetadata> {
        serde_json::from_str::<SplitMetadata>(&self.split_metadata_json).map_err(|err| {
            error!(
                index_id = %self.index_id, split_id = %self.split_id,
                "Failed to deserialize split metadata."
            );
            let message = format!(
                "Failed to deserialize split metadata. index_id=`{}`, split_id=`{}`.",
                self.index_id, self.split_id
            );
            MetastoreError::InternalError {
                message,
                cause: anyhow::anyhow!(err),
            }
        })
    }

    /// Deserializes and returns the split's state.
    fn split_state(&self) -> MetastoreResult<SplitState> {
        SplitState::from_str(&self.split_state).map_err(|err| {
            error!(
                index_id = %self.index_id, split_id = %self.split_id, split_state = %self.split_state,
                "Failed to deserialize split state."
            );
            let message = format!(
                "Failed to deserialize split state: `{}`. index_id=`{}`, split_id=`{}`.",
                self.split_state, self.index_id, self.split_id
            );
            MetastoreError::InternalError {
                message,
                cause: anyhow::anyhow!(err)
            }
        })
    }
}

impl TryInto<QuickwitSplit> for Split {
    type Error = MetastoreError;

    fn try_into(self) -> Result<QuickwitSplit, Self::Error> {
        let mut split_metadata = self.split_metadata()?;
        // `create_timestamp` is duplicated in `SplitMetadata` and needs to be overridden with the
        // "true" value stored in a column.
        split_metadata.create_timestamp = self.create_timestamp.timestamp();
        let split_state = self.split_state()?;
        let update_timestamp = self.update_timestamp.timestamp();
        Ok(QuickwitSplit {
            split_metadata,
            split_state,
            update_timestamp,
        })
    }
}
