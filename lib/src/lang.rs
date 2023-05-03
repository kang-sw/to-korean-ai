use compact_str::CompactString;

pub trait PromptProfile {
    fn proper_noun_instruction(&self) -> &str;

    /// 서버 응답을 파싱해서 고유명사 리스트 추출. Parse 실패 시 None
    fn parse_proper_noun_output(&self, response: &str) -> Option<Vec<CompactString>>;

    fn translate_instruction(&self) -> &str;
    fn translate_reference_1(&self) -> &str;
    fn translate_reference_2(&self) -> &str;
    fn translate_ready(&self) -> &str;
}

#[derive(Debug, serde::Deserialize, serde::Serialize, Clone, Copy, Default)]
pub enum Language {
    #[default]
    English,

    Korean,

    Japanese,
}

impl Language {
    pub fn to_english(&self) -> &'static str {
        match self {
            Language::English => "English",
            Language::Korean => "Korean",
            Language::Japanese => "Japanese",
        }
    }

    pub fn to_korean(&self) -> &'static str {
        match self {
            Language::English => "영어",
            Language::Korean => "한국어",
            Language::Japanese => "일본어",
        }
    }

    pub fn profile(&self) -> &'static dyn PromptProfile {
        match self {
            Language::English => unimplemented!(),
            Language::Japanese => unimplemented!(),
            Language::Korean => &profiles::ToKorean,
        }
    }
}

pub mod profiles {
    use default::default;
    use indoc::indoc;

    pub struct ToKorean;

    impl super::PromptProfile for ToKorean {
        fn proper_noun_instruction(&self) -> &str {
            indoc!(
                r##"제시된 일본어 원문으로부터,
                 
                 - "사람 이름"으로 추론되는 모든 단어를 추출하여 `person` 배열에 나열하십시오.
                 - "고유 명사 지명"으로 추론되는 모든 단어를 추출하여 `location` 배열에 나열하십시오.
                 
                 출력 형식은 YAML입니다."##
            )
        }

        fn parse_proper_noun_output(
            &self,
            response: &str,
        ) -> Option<Vec<compact_str::CompactString>> {
            dbg!(response);
            default()
        }

        fn translate_instruction(&self) -> &str {
            todo!()
        }

        fn translate_reference_1(&self) -> &str {
            todo!()
        }

        fn translate_reference_2(&self) -> &str {
            todo!()
        }

        fn translate_ready(&self) -> &str {
            todo!()
        }
    }
}
