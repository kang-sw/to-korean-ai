//!
//! 현재 열려 있는 책의 컨텍스트.
//!
//! - 책 콘텐트를 위한 SQLITE DB
//! - 책 설정 파일
//!

use std::path::Path;

pub struct Book {}

#[derive(config_it::Template, Debug, Clone)]
pub struct BookConfig {
    #[config]
    source_lang: String,

    #[config]
    alias: String,
}

/* --------------------------------------- Creation Logics -------------------------------------- */
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
        todo!()
    }
}

/* ----------------------------------------- Open Logic ----------------------------------------- */
#[derive(Debug, thiserror::Error)]
pub enum BookOpenError {}

impl Book {
    /// 이미 존재하는 책을 엽니다.
    pub fn open(library_dir: &Path, book_name: &str) -> Result<Book, BookOpenError> {
        todo!()
    }
}

/* --------------------------------------- Content Update --------------------------------------- */

/* ---------------------------------------- Content Seek ---------------------------------------- */
