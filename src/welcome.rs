//! The browser build's first screen: what csvr is, how to open a file, the main keys and the
//! licenses. It stays until a file or the sample opens, and `WebRoot` then shows the table in
//! its place.
//!
//! The text here must not make the page download the Japanese font (about 7MB). gpui_web
//! (v1.27.0, `is_cjk` in `gpui_web/src/canvas_fallback.rs`) draws kanji, kana, Hangul and the
//! punctuation `、。〈〉《》「」『』【】〔〕（）［］｛｝，．！？：；・ー々〆` with the browser's font,
//! and the bundled IBM Plex Sans draws ASCII. Any other character (`〜`, `①`, `※`, `→`, `×`,
//! full-width letters and digits, ...) is reported as missing and loads the font each time the
//! page opens, so use ASCII (`->`, `+`) for those. Check this again when gpui_web changes.

use gpui::prelude::*;
use gpui::{
    AnyElement, AppContext, Context, Entity, FocusHandle, FontWeight, SharedString, Window, div,
    px, rems, rgb,
};

use csvr_core::data::CsvData;

use crate::app::{
    BG_BASE, BORDER_COLOR, CsvrApp, OpenFile, SEARCH_BG, SURFACE1, TABLE_FONT_FAMILY, TEXT_ERROR,
    TEXT_MAIN, TEXT_SUBTEXT,
};

/// The key context of the welcome screen. It has no text input, so unlike the table it needs no
/// `!ime_editing`.
pub(crate) const WELCOME_KEY_CONTEXT: &str = "Welcome";

const SAMPLE_CSV: &str = "\
名前,部署,年齢,売上
田中,営業,34,1200
佐藤,開発,28,860
鈴木,営業,45,1530
高橋,総務,39,720
伊藤,開発,31,990
";

/// The main keys, as `key_bindings` and `CsvrApp`'s `on_key_down` handle them. Only `cmd` is
/// listed; a line below the table says `ctrl` works too.
const KEYS: [(&str, &str); 10] = [
    ("/ または Cmd+F", "検索（文字を含む行に絞り込む）"),
    ("&", "行を正規表現で絞り込む（列名:パターン で列を指定）"),
    ("*", "列名の正規表現で、表示する列を選ぶ"),
    ("f", "左から指定した数の列を固定"),
    ("ヘッダーをクリック", "並べ替え（昇順、降順、元の順）"),
    ("クリック、矢印キー", "セルを選ぶ"),
    ("Cmd+C", "選んだセルや行をコピー"),
    ("Cmd+Shift+J / M", "表示中の表を JSON / Markdown でコピー"),
    ("Cmd+G", "グラフ"),
    ("Esc", "入力欄を閉じる、選択を解除"),
];

/// The browser build's window root: the welcome screen until something opens, then the table.
pub(crate) struct WebRoot {
    screen: Screen,
    welcome_focus: FocusHandle,
    /// Why the last file did not open, shown on the welcome screen. The table shows it in its
    /// status bar instead.
    message: Option<SharedString>,
    /// Numbers each file or sample opened. Reads finish in any order, so only the latest may
    /// show; otherwise a large file dropped first could finish last and cover what the user
    /// opened afterwards.
    latest_load: u64,
}

enum Screen {
    Welcome,
    Table(Entity<CsvrApp>),
}

impl WebRoot {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        Self {
            screen: Screen::Welcome,
            welcome_focus: cx.focus_handle(),
            message: None,
            latest_load: 0,
        }
    }

    pub(crate) fn welcome_focus(&self) -> &FocusHandle {
        &self.welcome_focus
    }

    /// Starts a load and returns its number. Loads started before it no longer show. It also
    /// clears the last failure, which would otherwise stay on the welcome screen while the next
    /// file is read and look like that file failed.
    pub(crate) fn start_load(&mut self, cx: &mut Context<Self>) -> u64 {
        self.latest_load += 1;
        if self.message.take().is_some() {
            cx.notify();
        }
        self.latest_load
    }

    pub(crate) fn is_latest_load(&self, id: u64) -> bool {
        self.latest_load == id
    }

    /// The table, if one is open.
    pub(crate) fn table(&self) -> Option<&Entity<CsvrApp>> {
        match &self.screen {
            Screen::Welcome => None,
            Screen::Table(table) => Some(table),
        }
    }

    pub(crate) fn show_table(
        &mut self,
        data: CsvData,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let table = cx.new(|cx| CsvrApp::new(data, window, cx));
        let focus = table.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
        self.screen = Screen::Table(table);
        self.message = None;
        cx.notify();
    }

    /// Shows a message where the current screen has room for it.
    pub(crate) fn show_message(&mut self, message: String, cx: &mut Context<Self>) {
        match &self.screen {
            Screen::Table(table) => {
                table.update(cx, |app, cx| app.show_status_message(message, cx));
            }
            Screen::Welcome => {
                self.message = Some(message.into());
                cx.notify();
            }
        }
    }

    fn open_sample(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A file still being read must not replace the sample when it finishes.
        self.start_load(cx);
        let sample = CsvData::from_reader(SAMPLE_CSV.as_bytes()).expect("the sample CSV is valid");
        self.show_table(sample, window, cx);
    }

    fn render_welcome(&self, cx: &mut Context<Self>) -> AnyElement {
        let buttons = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap_2()
            .child(
                button("open-file", "ファイルを開く").on_click(|_event, window, cx| {
                    // `web` opens the file picker; the welcome screen cannot call it.
                    window.dispatch_action(Box::new(OpenFile), cx);
                }),
            )
            .child(
                button("open-sample", "サンプルを開く")
                    .on_click(cx.listener(|this, _event, window, cx| this.open_sample(window, cx))),
            );
        let keys = div()
            .flex()
            .flex_col()
            .gap_1()
            .pt_3()
            .border_t_1()
            .border_color(rgb(BORDER_COLOR))
            .children(KEYS.iter().map(|(key, action)| {
                div()
                    .flex()
                    .flex_row()
                    .gap_3()
                    .child(div().w(rems(9.)).flex_shrink_0().child(*key))
                    .child(div().flex_1().text_color(rgb(TEXT_SUBTEXT)).child(*action))
            }))
            .child(
                div()
                    .pt_1()
                    .text_color(rgb(TEXT_SUBTEXT))
                    .child("Cmd は Ctrl でも使えます。"),
            );
        let licenses = div()
            .id("licenses")
            .text_color(rgb(TEXT_SUBTEXT))
            .underline()
            .cursor_pointer()
            .hover(|style| style.text_color(rgb(TEXT_MAIN)))
            // gpui_web opens it in a new tab, so the page and its state stay.
            .on_click(|_event, _window, cx| cx.open_url("licenses/"))
            .child("ライセンス");
        div()
            .id("welcome")
            .track_focus(&self.welcome_focus)
            .key_context(WELCOME_KEY_CONTEXT)
            .size_full()
            .overflow_y_scroll()
            .font_family(TABLE_FONT_FAMILY)
            .bg(rgb(BG_BASE))
            .text_color(rgb(TEXT_MAIN))
            .text_sm()
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(600.))
                    // Centred while it fits; auto margins, unlike justify_center, keep the top
                    // reachable by scrolling when the window is shorter than the content.
                    .my_auto()
                    .px_4()
                    .py_8()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_2xl().font_weight(FontWeight::BOLD).child("csvr"))
                            .child(
                                div()
                                    .text_color(rgb(TEXT_SUBTEXT))
                                    .child("CSV と Excel (xlsx / xls) を表で見る"),
                            ),
                    )
                    .child(buttons)
                    .when_some(self.message.clone(), |el, message| {
                        el.child(div().text_color(rgb(TEXT_ERROR)).child(message))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .text_color(rgb(TEXT_SUBTEXT))
                            .child(
                                "この画面にドロップしても開けます（Cmd+O / Ctrl+O でも開けます）。",
                            )
                            .child("ファイルはブラウザの中で読み込み、外には送りません。"),
                    )
                    .child(keys)
                    .child(licenses),
            )
            .into_any_element()
    }
}

/// The colors and hover of the table's Open file button, with more padding for the welcome
/// screen.
fn button(id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_3()
        .py_1()
        .rounded_md()
        .bg(rgb(SEARCH_BG))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(SURFACE1)))
        .child(label)
}

impl Render for WebRoot {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.screen {
            Screen::Welcome => self.render_welcome(cx),
            Screen::Table(table) => table.clone().into_any_element(),
        }
    }
}
