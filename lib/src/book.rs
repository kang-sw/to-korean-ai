//!
//! 현재 열려 있는 책의 컨텍스트.
//!
//! - 책 콘텐트를 위한 SQLITE DB
//! - 책 설정 파일
//!

use std::path::Path;

use compact_str::CompactString;

use crate::lang::Language;

#[derive(Debug)]
pub struct Book {
    db: rusqlite::Connection,
}

#[derive(config_it::Template, Debug, Clone)]
pub struct BookConfig {
    #[config]
    source_lang: Language,

    #[config]
    alias: String,
}

pub struct Line {
    pub line_number: usize,
    pub source: String,
    pub translation_id: Option<usize>,
    pub state: LineState,
}

pub struct TranslatedLine {
    pub id: u64,
    pub line_number: usize,
    pub ntok_prompt: usize,
    pub ntok_reply: usize,
    pub content: String,
}

pub enum LineState {
    ImplicitConfirm = 0,
    ExplicitConfirm = 1,
    Invalidated = 2,
    InvalidateAllAfter = 3,
}

/* ---------------------------------------------------------------------------------------------- */
/*                                            CREATION                                            */
/* ---------------------------------------------------------------------------------------------- */
#[derive(Debug, thiserror::Error)]
pub enum BookCreationError {}

impl Book {
    pub fn create<'a, T, G: 'a>(
        library_dir: &Path,
        book_name: &str,
        content: T,
    ) -> Result<Book, BookCreationError>
    where
        T: IntoIterator<Item = &'a G>,
        G: AsRef<str>,
    {
        Self::_create_book(
            library_dir,
            book_name,
            &mut content.into_iter().map(|x| x.as_ref()),
        )
    }

    fn _create_book(
        library_dir: &Path,
        book_name: &str,
        content: &mut dyn Iterator<Item = &str>,
    ) -> Result<Book, BookCreationError> {
        // 1. 폴더 생성 -> book_name은 base64로 컨버전 후 기록
        // 2. 텍스트 모두 임포트 -> 행 트리밍, 빈 행 삭제 등 작업 수행
        // 3. 책 관련 파일 생성: DB, Config, ...
        // 4. book.info 파일 생성

        todo!()
    }
}

/* ----------------------------------------- Open Logic ----------------------------------------- */
#[derive(Debug, thiserror::Error)]
pub enum BookOpenError {}

impl Book {
    /// 이미 존재하는 책을 엽니다.
    pub fn open(library_dir: &Path, book_name: &str) -> Result<Book, BookOpenError> {
        // 1.

        todo!()
    }

    /// 폴더 내 모든 책 이름을 열거합니다.
    pub fn list(library_dir: &Path) -> std::io::Result<Vec<String>> {
        todo!()
    }
}

/* ---------------------------------------------------------------------------------------------- */
/*                                           DICTIONARY                                           */
/* ---------------------------------------------------------------------------------------------- */

/* --------------------------------------- Dict - Lookups --------------------------------------- */
#[derive(thiserror::Error, Debug)]
pub enum DictionaryUpdateError {}

impl Book {
    /// 책의 사전을 검색합니다.
    pub async fn dict_lookup<'a>(
        &self,
        detected_names: impl IntoIterator<Item = &'a str> + ExactSizeIterator,
    ) -> Vec<(CompactString, CompactString)> {
        todo!()
    }

    pub async fn dict_update(
        &self,
        name: &str,
        translation: &str,
    ) -> Result<(), DictionaryUpdateError> {
        todo!()
    }
}

/* ---------------------------------------------------------------------------------------------- */
/*                                            CONTENTS                                            */
/* ---------------------------------------------------------------------------------------------- */
