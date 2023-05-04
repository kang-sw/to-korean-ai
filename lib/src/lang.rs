use async_openai::types::Role;
use compact_str::CompactString;
use std::borrow::Cow;

/// 각종 시스템 프롬프트를 생성하기 위한 프로필입니다.
pub trait PromptProfile: Send + Sync + std::fmt::Debug {
    fn proper_noun_instruction(&self, src_lang: Language) -> Cow<str>;

    /// 서버 응답을 파싱해서 고유명사 리스트 추출. Parse 실패 시 None
    fn parse_proper_noun_output(
        &self,
        src_lang: Language,
        response: &str,
    ) -> Option<Vec<CompactString>>;

    /// 번역 단계에서 제공: 셋업 시스템 프롬프트입니다.
    fn translation(
        &self,
        src_lang: Language,
        dict: &[(&str, &str)],
        pre_ctx: &str,
        content: &str,
    ) -> Vec<(Role, Cow<str>)>;
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
}

pub mod profiles {
    #[derive(Debug)]
    pub struct KoreanV1;

    mod korean_v1 {
        use std::borrow::Cow;

        use async_openai::types::Role;
        use compact_str::CompactString;
        use lazy_static::lazy_static;

        use super::KoreanV1;
        use crate::lang::Language;

        macro_rules! static_lang_str_ko {
            ($str:expr, $source_lang:expr) => {{
                const FMT_BASE: &str = indoc::indoc!($str);

                lazy_static! {
                    static ref JP: String = FMT_BASE.replace("{{LANG}}", "일본어");
                    static ref EN: String = FMT_BASE.replace("{{LANG}}", "영어");
                };

                match $source_lang {
                    Language::English => EN.as_str(),
                    Language::Japanese => JP.as_str(),
                    Language::Korean => panic!("Korean -> Korean not allowed!"),
                }
            }};
        }

        impl super::super::PromptProfile for KoreanV1 {
            fn proper_noun_instruction(&self, source_lang: Language) -> Cow<str> {
                static_lang_str_ko!(
                    r##"    제시된 {{LANG}} 원문으로부터,
                 
                            - "사람 이름"으로 추론되는 모든 단어를 추출하여 `person` 배열에 나열하십시오.
                            - "고유 명사 지명"으로 추론되는 모든 단어를 추출하여 `location` 배열에 나열하십시오.
                    
                            출력 형식은 YAML입니다."##,
                    source_lang
                ).into()
            }

            fn parse_proper_noun_output(
                &self,
                _source_lang: Language,
                response: &str,
            ) -> Option<Vec<compact_str::CompactString>> {
                log::debug!("parsing noun output: {:?}", response);

                // 마크다운 문법으로 YAML 받음 ... 먼저 앞뒤의 ```yaml ~~ ``` 떼어낸다.
                let lines: Vec<_> = response
                    .lines()
                    .skip_while(|line| line.starts_with("```yaml") == false)
                    .skip(1)
                    .collect();

                let close_pos = lines
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, line)| line.starts_with("```"))
                    .map(|(pos, _)| pos)?;

                let lines = &lines[..close_pos];

                #[derive(serde::Deserialize)]
                struct Output<'a> {
                    #[serde(borrow)]
                    person: Vec<&'a str>,
                    #[serde(borrow)]
                    location: Vec<&'a str>,
                }

                let rebuilt_text = lines.join("\n");
                let output: Output<'_> = serde_yaml::from_str(&rebuilt_text).ok()?;

                Some(
                    output
                        .person
                        .into_iter()
                        .chain(output.location.into_iter())
                        .map(|x| CompactString::new(x))
                        .collect(),
                )
            }

            fn translation(
                &self,
                src_lang: Language,
                dict: &[(&str, &str)],
                pre_ctx: &str,
                content: &str,
            ) -> Vec<(Role, Cow<str>)> {
                let mut ret = Vec::with_capacity(10);
                let lang = src_lang.to_korean();

                ret.push((
                    Role::System,
                    format!("지시사항에 따라 동작한다. 반드시 지시된 내용만 출력한다.").into(),
                ));

                if pre_ctx.is_empty() == false {
                    ret.push((
                        Role::User,
                        format!(
                            concat!(
                                "**지시사항 \n\n",
                                "다음 {lang} 원문의 내용을 먼저 이해하십시오.\n\n",
                                "[[[시작]]]\n\n{content}\n\n[[[끝]]]\n\n",
                                "**출력 \n\n내용을 이해했다면 '네'라고 대답하십시오."
                            ),
                            lang = lang,
                            content = pre_ctx,
                        )
                        .into(),
                    ));

                    ret.push((Role::Assistant, format!("네.").into()));
                }

                if dict.is_empty() == false {
                    // TODO: 사전 프롬프트 출력하기
                }

                ret.push((
                    Role::User,
                    format!(
                        concat!(
                            "**지시사항\n\n",
                            "다음 규칙을 바탕으로 {lang} 문장을 한국어로 번역하십시오.\n\n",
                            "- 번역 과정에서 존댓말의 사용을 최소화한다.\n",
                            "- 화폐의 단위를 변경하지 않는다.\n",
                            "- 문장을 생략하거나 줄이지 않는다.\n",
                            "- 각 입력 라인이 각 출력 라인에 대응되어야 한다.\n",
                            "- 각 문장을 올바르고 자연스러운 한국어로 교정한다.\n",
                            "- 반드시 한국어로 출력한다.\n",
                            "\n\n",
                            "**{lang} 원문\n\n",
                            "{content}",
                            "\n\n\n",
                            "**출력\n\n",
                            "제공된 '{lang} 원문'의 \"한국어\" 번역본을 출력하십시오.\n\n",
                        ),
                        lang = lang,
                        content = content
                    )
                    .into(),
                ));

                ret
            }
        }
    }
}
