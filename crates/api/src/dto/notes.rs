use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use senken_notes::{Note, NoteSummary};

/// A note row without its body, as returned by a listing — see [`NoteDto`]
/// for the full row.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct NoteSummaryDto {
    /// The note's id.
    pub id: String,
    /// The note's title.
    pub title: String,
    /// Unix timestamp of the last change to the note's title or body.
    pub updated_at: i64,
}

impl From<NoteSummary> for NoteSummaryDto {
    fn from(summary: NoteSummary) -> Self {
        Self {
            id: summary.id.to_string(),
            title: summary.title,
            updated_at: summary.updated_at,
        }
    }
}

/// `GET /api/notes` response body (scope reaches the query, including this `total`; body-free, see [`NoteSummaryDto`]).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct NotesPage {
    /// The rows for this page.
    pub rows: Vec<NoteSummaryDto>,
    /// How many rows exist in total, under the same scope as `rows`.
    pub total: u64,
}

/// A full note row, body included — `GET /api/notes/{note_id}` only.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct NoteDto {
    /// The note's id.
    pub id: String,
    /// The note's title.
    pub title: String,
    /// The note's body.
    pub body: String,
    /// Unix timestamp of creation.
    pub created_at: i64,
    /// Unix timestamp of the last change to the note's title or body.
    pub updated_at: i64,
}

impl From<Note> for NoteDto {
    fn from(note: Note) -> Self {
        Self {
            id: note.id.to_string(),
            title: note.title,
            body: note.body,
            created_at: note.created_at,
            updated_at: note.updated_at,
        }
    }
}

/// `POST /api/notes` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct CreateNoteRequest {
    /// The new note's title.
    pub title: String,
    /// The new note's body.
    pub body: String,
}

/// `PUT /api/notes/{note_id}` request body: replaces both fields, the same
/// "full replace" shape `ReplaceLayoutRequest` uses.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct UpdateNoteRequest {
    /// The note's new title.
    pub title: String,
    /// The note's new body.
    pub body: String,
}
