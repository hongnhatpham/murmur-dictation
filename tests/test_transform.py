from murmur.transform import PRESS_ENTER_AFTER_INSERT, clean_text, detect_press_enter, transform_transcript


def test_detect_press_enter_trailing_phrase():
    text, action = detect_press_enter("hello world press enter")
    assert text == "hello world"
    assert action is True


def test_detect_press_enter_with_punctuation_and_case():
    text, action = detect_press_enter("Hello world, PRESS ENTER.")
    assert text == "Hello world"
    assert action is True


def test_detect_press_enter_only_phrase():
    text, action = detect_press_enter("press enter")
    assert text == ""
    assert action is True


def test_transform_clean_strips_press_enter_and_emits_action():
    result = transform_transcript("uh hello there press enter", mode="clean")
    assert result.final_text == "Hello there."
    assert [action.name for action in result.actions] == [PRESS_ENTER_AFTER_INSERT]


def test_raw_mode_preserves_press_enter_literal():
    result = transform_transcript("hello press enter", mode="raw")
    assert result.final_text == "hello press enter"
    assert result.actions == []


def test_clean_text_capitalizes_and_punctuates():
    assert clean_text("um this is fine") == "This is fine."
