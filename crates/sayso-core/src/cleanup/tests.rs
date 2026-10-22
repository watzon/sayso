use super::*;

fn s1() -> &'static CleanupModelInfo {
    find("s1-mini").unwrap()
}

fn spokenly() -> &'static CleanupModelInfo {
    find("spokenly-cleanup-1").unwrap()
}

#[test]
fn s1_mini_gets_the_exact_format_of_its_model_card() {
    let p = render(s1(), "so um send the report by uh friday", Settings::default(), Some("Slack"), None, 10);
    assert_eq!(
        p.text,
        "<|im_start|>system\nYou are a text normalizer for speech-to-text transcripts. The input begins with a control line \
         specifying the styling, structure, and context settings; clean the transcript to match those settings and output only \
         the cleaned text.<|im_end|>\n<|im_start|>user\n[Styling: semi-formal] [Structure: prose] [Context: general]\n\
         so um send the report by uh friday<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"
    );
    assert_eq!(p.end, vec!["<|im_end|>".to_string()]);
    assert_eq!(p.max_tokens, 45);
    // No app line: S1-mini reads no context.
    assert!(!p.text.contains("Slack"));
    // A casual tone does not reach S1-mini (the evaluation: it keeps filler words).
    let casual = render(s1(), "x", Settings { tone: Tone::VeryCasual, ..Default::default() }, None, None, 1);
    assert!(casual.text.contains("[Styling: semi-formal]"));
}

#[test]
fn s1_mini_gets_lists_and_email_from_the_settings() {
    let list = render(s1(), "x", Settings { layout: Layout::List, ..Default::default() }, None, None, 1);
    assert!(list.text.contains("[Styling: semi-formal] [Structure: lists] [Context: general]\nx<|im_end|>"));
    let email = render(s1(), "x", Settings { format: Format::Email, ..Default::default() }, None, None, 1);
    assert!(email.text.contains("[Structure: prose] [Context: email]"));
}

#[test]
fn cleanup_1_gets_key_value_lines_in_the_order_of_its_model_card() {
    let p = render(spokenly(), "hey are we still on", Settings { tone: Tone::Casual, ..Default::default() }, Some("Slack"), Some("app.slack.com"), 5);
    assert_eq!(p.text, "app: Slack\nsite: app.slack.com\ntone: casual\ntext: hey are we still on\nout:");
    let plain = render(spokenly(), "um hi", Settings::default(), None, Some("  "), 2);
    assert_eq!(plain.text, "tone: formal\ntext: um hi\nout:");
    let very = render(spokenly(), "x", Settings { tone: Tone::VeryCasual, ..Default::default() }, Some("My\nApp"), None, 1);
    assert_eq!(very.text, "app: My App\ntone: very casual\ntext: x\nout:");
}

#[test]
fn runs_matches_the_table_of_the_plan() {
    let clean = CleanupSettings::new(Tone::Formal, Layout::Paragraphs, Format::Plain);
    let message = CleanupSettings::new(Tone::Casual, Layout::Paragraphs, Format::Plain);
    let email = CleanupSettings::new(Tone::Formal, Layout::Paragraphs, Format::Email);
    let notes = CleanupSettings::new(Tone::Formal, Layout::List, Format::Plain);
    for (settings, on_s1, on_spokenly) in [(&clean, true, true), (&message, true, true), (&email, true, false), (&notes, true, false)] {
        assert_eq!(s1().runs(settings), on_s1, "{settings:?}");
        assert_eq!(spokenly().runs(settings), on_spokenly, "{settings:?}");
    }
    // A value from a newer Sayso: no model runs the style, and nothing panics.
    let newer = CleanupSettings { layout: Some("table".into()), ..Default::default() };
    assert!(!s1().runs(&newer) && !spokenly().runs(&newer));
    assert_eq!(newer.known(), None);
}

#[test]
fn settings_write_only_values_that_are_not_the_default() {
    assert_eq!(CleanupSettings::new(Tone::Formal, Layout::Paragraphs, Format::Plain), CleanupSettings::default());
    let email = CleanupSettings::new(Tone::Formal, Layout::Paragraphs, Format::Email);
    assert_eq!(toml::to_string(&email).unwrap().trim(), r#"format = "email""#);
    let parsed: CleanupSettings = toml::from_str("tone = \"very-casual\"").unwrap();
    assert_eq!(parsed.known().unwrap().tone, Tone::VeryCasual);
}

#[test]
fn the_catalog_pins_files_by_revision_and_hash() {
    for model in catalog() {
        assert_eq!(model.revision.len(), 40, "{}", model.id);
        assert_eq!(model.sha256.len(), 64, "{}", model.id);
        assert!(model.file_url(model.file).contains(model.revision));
        assert!(model.supports_language("en"));
    }
    assert!(!s1().supports_language("de"));
    assert!(spokenly().supports_language("de"));
}

/// One token per four bytes, like a rough tokenizer.
fn by_bytes(text: &str) -> Result<u32, ()> {
    Ok(text.len().div_ceil(4) as u32)
}

fn plan(text: &str, max: u32) -> Vec<String> {
    plan_parts(text, max, &mut by_bytes).unwrap()
}

fn assert_whole(text: &str, parts: &[String]) {
    assert_eq!(parts.concat(), text, "every character is in one part, in order");
    assert!(parts.iter().all(|p| !p.is_empty()));
}

#[test]
fn a_short_text_is_one_part() {
    let parts = plan("Hello there.", 100);
    assert_eq!(parts, vec!["Hello there.".to_string()]);
}

#[test]
fn a_long_text_splits_after_sentence_ends() {
    let text = "one two three four. five six seven eight. nine ten eleven twelve. thirteen fourteen.";
    let parts = plan(text, 12);
    assert_whole(text, &parts);
    assert!(parts.len() > 1);
    for part in &parts[..parts.len() - 1] {
        assert!(part.trim_end().ends_with('.'), "{part:?}");
        assert!(by_bytes(part).unwrap() <= 12);
    }
}

#[test]
fn a_text_with_no_sentence_end_splits_at_commas_then_spaces() {
    let commas = "alpha beta gamma, delta epsilon zeta, eta theta iota, kappa lambda mu";
    let parts = plan(commas, 10);
    assert_whole(commas, &parts);
    assert!(parts[0].trim_end().ends_with(','), "{parts:?}");
    let words = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi";
    let parts = plan(words, 6);
    assert_whole(words, &parts);
    assert!(parts.iter().all(|p| by_bytes(p).unwrap() <= 6), "{parts:?}");
}

#[test]
fn chinese_text_with_no_spaces_splits_at_characters() {
    let text = "这是一个很长的句子没有任何标点符号也没有空格所以只能按字符切分";
    let parts = plan(text, 5);
    assert_whole(text, &parts);
    assert!(parts.len() > 3);
    assert!(parts.iter().all(|p| by_bytes(p).unwrap() <= 5), "{parts:?}");
    let with_marks = "第一句话。第二句话。第三句话。第四句话。";
    let parts = plan(with_marks, 8);
    assert_whole(with_marks, &parts);
    assert!(parts[0].ends_with('。'));
}

#[test]
fn new_lines_stay_with_the_part_before_and_come_back_in_the_join() {
    let text = "first line here\nsecond line here\nthird line here";
    let parts = plan(text, 5);
    assert_whole(text, &parts);
    assert!(parts[0].ends_with('\n'));
    let answers: Vec<String> = parts.iter().map(|p| p.trim().to_uppercase()).collect();
    let joined = join_answers(&parts, &answers);
    assert!(joined.contains("FIRST LINE HERE\nSECOND"), "{joined:?}");
    let sentences = vec!["One. ".to_string(), "Two.".to_string()];
    assert_eq!(join_answers(&sentences, &["One.".into(), "Two.".into()]), "One. Two.");
    // A part with only filler words gives an empty answer, and no double space.
    assert_eq!(join_answers(&["um. ".into(), "Hi.".into()], &["".into(), "Hi.".into()]), "Hi.");
}

#[test]
fn only_an_answer_that_ended_is_used() {
    assert_eq!(check("um hi there", Stop::End, " Hi there.\n"), Ok("Hi there.".into()));
    assert_eq!(check("um hi there", Stop::Limit, "Hi there"), Err(CheckFailure::Cut(Stop::Limit)));
    assert_eq!(check("um hi there", Stop::Context, "Hi"), Err(CheckFailure::Cut(Stop::Context)));
    assert_eq!(check("hi", Stop::End, "Hi.<|im_end|>"), Ok("Hi.".into()));
    assert_eq!(check("hi", Stop::End, "<think>\n\n</think>\n\nHi."), Ok("Hi.".into()));
}

#[test]
fn an_empty_answer_is_right_only_for_filler_words() {
    assert_eq!(check("um uh you know", Stop::End, ""), Ok(String::new()));
    assert_eq!(check("Äh ähm.", Stop::End, " "), Ok(String::new()));
    assert_eq!(check("send the report by friday", Stop::End, ""), Err(CheckFailure::Empty));
    assert!(!is_filler_only("you"), "you alone is a word");
    assert!(!is_filler_only(""));
    assert!(is_filler_only("えっと、あのー"));
}

#[test]
fn spoken_numbers_that_become_digits_are_not_too_short() {
    let input = "the total is nine hundred and ninety nine thousand nine hundred and ninety nine dollars";
    assert!(check(input, Stop::End, "The total is $999,999.").is_ok());
}

#[test]
fn an_answer_that_drops_most_of_the_text_fails() {
    let input = "so the plan for next week is that we finish the import screen on monday and test it on tuesday";
    assert_eq!(check(input, Stop::End, "So the plan."), Err(CheckFailure::TooShort));
    let chinese = "我们下周的计划是周一完成导入界面然后周二用三个大文件测试它最后写发布说明";
    assert_eq!(check(chinese, Stop::End, "我们下周。"), Err(CheckFailure::TooShort));
    assert!(check(chinese, Stop::End, "我们下周的计划是：周一完成导入界面，然后周二用三个大文件测试它，最后写发布说明。").is_ok());
    assert_eq!(check("hi", Stop::End, &"word ".repeat(100)), Err(CheckFailure::TooLong));
}

fn terms(list: &[&str]) -> Vec<String> {
    list.iter().map(|t| t.to_string()).collect()
}

#[test]
fn spelling_comes_back_only_for_terms_that_were_sent_with_it() {
    let t = terms(&["GPUI", "GitHub", "US"]);
    assert_eq!(restore_spelling("open the GPUI docs", "Open the Gpui docs.", &t), "Open the GPUI docs.");
    assert_eq!(restore_spelling("push it to GitHub", "push it to github", &t), "push it to GitHub");
    // "US" was not in the text as "US", so "us" stays.
    assert_eq!(restore_spelling("tell us more", "Tell us more.", &t), "Tell us more.");
    // Whole words only.
    assert_eq!(restore_spelling("GPUI", "gpuis are fun", &t), "gpuis are fun");
}

#[test]
fn spelling_takes_longer_terms_first_and_handles_unicode() {
    let t = terms(&["Claude", "Claude Code", "Zoë"]);
    assert_eq!(restore_spelling("ask Claude Code and Zoë", "Ask claude code and zoë.", &t), "Ask Claude Code and Zoë.");
    // A very casual answer still gets names back.
    assert_eq!(restore_spelling("ask Claude", "ask claude", &t), "ask Claude");
}

#[test]
fn the_language_comes_from_the_setting_then_the_model_then_the_text() {
    let never = |_: &str| -> Option<String> { panic!("no detection needed") };
    assert_eq!(dictation_language("de", &["en", "de"], "x", never).as_deref(), Some("de"));
    assert_eq!(dictation_language("auto", &["en"], "x", never).as_deref(), Some("en"));
    assert_eq!(dictation_language("auto", &["en", "de"], "Guten Tag", |_| Some("de".into())).as_deref(), Some("de"));
    assert_eq!(dictation_language("auto", &["en", "de"], "ok", |_| None), None);
}

#[test]
fn content_units_count_a_number_once() {
    assert_eq!(content_units("nine hundred and ninety nine dollars"), 2);
    assert_eq!(content_units("$999,999 at 5:30 PM"), 4);
    assert_eq!(content_units("bread and butter"), 3);
    assert_eq!(content_units("我们下周"), 4);
    assert_eq!(content_units("Wir sehen uns um 3 Uhr."), 6);
}

#[test]
fn the_detector_gives_a_language_only_when_it_is_sure() {
    assert_eq!(detect_language("We need to fix the login bug and update the docs before the release on Friday.").as_deref(), Some("en"));
    assert_eq!(detect_language("Wir müssen den Fehler beim Anmelden beheben und die Dokumentation vor Freitag aktualisieren.").as_deref(), Some("de"));
    assert_eq!(detect_language("ok"), None);
}

#[test]
fn a_count_error_ends_the_split_at_once() {
    let mut calls = 0;
    let result: Result<Vec<String>, &str> = plan_parts(&"word ".repeat(500), 10, &mut |_| {
        calls += 1;
        Err("the runtime stopped")
    });
    assert_eq!(result, Err("the runtime stopped"));
    assert_eq!(calls, 1);
}

#[test]
fn spelling_stays_when_the_text_had_the_word_in_two_cases() {
    let t = terms(&["May", "US"]);
    assert_eq!(restore_spelling("we may ship in May", "We may ship in May.", &t), "We may ship in May.");
    assert_eq!(restore_spelling("the US and us", "The us and us.", &t), "The us and us.");
    assert_eq!(restore_spelling("ship in May", "Ship in may.", &t), "Ship in May.");
}
