//! 設定画面「プロファイル」タブ（Issue #721）
//!
//! `tako master` / `tako solo` の起動プロファイル（profiles/*.yaml /
//! solo-profiles/*.yaml）を GUI で確認・編集・作成する。
//!
//! 設計の要点:
//! - **書き込みは必ず `Request::OrchestratorProfiles` 経由**。yaml を UI から直接
//!   書かないので、#169 の config_io（ロック + アトミック書き込み + 世代バックアップ）と
//!   CLI / MCP の検証がそのまま効く（開発不変条件の 1:1 が構造的に成立する）
//! - 選択肢が既知の項目（エージェント種別・effort・ポリシー・アカウント・プロジェクト）は
//!   **チップの選択式**にして自由入力を排除する。ポップアップ方式を採らないのは、
//!   タブ本文が `overflow_y_scroll` でクリップされる（#321 と同じ構造）ため
//! - モデル名は上流 CLI のリリースごとに変わるので既知の選択肢を持てない。自由入力 +
//!   「既定に戻す」（= model 未指定 = CLI 既定。#27 の推奨）にしている
//! - 種別の切替・タブ表示のたびに dispatch で読み直すので、CLI / MCP / 手編集の
//!   変更にも追随する（#486 と同じ方針）

use gpui::prelude::FluentBuilder;
use gpui::*;
use tako_control::orchestrator::{self, ProfileKind, WorkerAgent};
use tako_control::protocol::Request;

use super::{to_hsla, BtnKind, EditField, SettingsWindow};
use crate::ui_text::settings as txt;

/// 種別の表示名（UI 文言。判定・パス解決は tako-control の ProfileKind が正）
fn kind_label(kind: ProfileKind) -> &'static str {
    match kind {
        ProfileKind::Master => txt::prof_kind_master(),
        ProfileKind::Solo => txt::prof_kind_solo(),
    }
}

/// プロファイルタブのテキスト入力欄。`EditField::Profile` の中身
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileField {
    /// 新規作成・複製で使う名前
    NewName,
    Model,
    WorkerModel,
    TabNaming,
    /// エージェント別設定のモデル（対象エージェント名）
    AgentModel(String),
    /// エージェント別設定の追加引数（対象エージェント名）
    AgentArgs(String),
    EnvKey,
    EnvValue,
}

impl ProfileField {
    pub fn slug(&self) -> String {
        match self {
            Self::NewName => "prof-new-name".into(),
            Self::Model => "prof-model".into(),
            Self::WorkerModel => "prof-worker-model".into(),
            Self::TabNaming => "prof-tab-naming".into(),
            Self::AgentModel(a) => format!("prof-agent-model-{a}"),
            Self::AgentArgs(a) => format!("prof-agent-args-{a}"),
            Self::EnvKey => "prof-env-key".into(),
            Self::EnvValue => "prof-env-value".into(),
        }
    }
}

/// プロファイルタブの状態。SettingsWindow へ 1 フィールドで持たせる
#[derive(Default)]
pub struct ProfilesTabState {
    pub kind: ProfileKind,
    /// 一覧（dispatch list の profiles 配列）
    list: Vec<serde_json::Value>,
    /// 選択中のプロファイル名
    selected: Option<String>,
    /// 選択中プロファイルの詳細（dispatch show の結果）
    detail: Option<serde_json::Value>,
    /// 登録済みアカウント名
    accounts: Vec<String>,
    /// 登録済みプロジェクトキー
    projects: Vec<String>,
    /// エージェント別設定で編集中の対象
    agent_target: String,
    /// 削除確認中のプロファイル名
    confirm_delete: Option<String>,
    /// 新規作成 / 複製の名前バッファ
    new_name: String,
    /// 環境変数追加の入力バッファ
    env_key: String,
    env_value: String,
}

impl SettingsWindow {
    // --- 状態の取得（タブ表示・種別切替・操作後に呼ぶ）---

    /// プロファイルタブの表示に必要な状態を dispatch から読み直す。
    /// 一覧 → 選択の確定 → 詳細 → 選択肢（アカウント / プロジェクト）の順
    pub(super) fn refresh_profiles(&mut self, cx: &mut Context<Self>) {
        let kind = self.profiles.kind;
        let list = self
            .query(profiles_request("list", kind, None), cx)
            .and_then(|v| v["profiles"].as_array().cloned())
            .unwrap_or_default();

        // 選択中のプロファイルが消えていたら先頭へ戻す（CLI からの削除に追随）
        let names: Vec<String> = list
            .iter()
            .filter_map(|p| p["name"].as_str().map(String::from))
            .collect();
        let selected = match self.profiles.selected.clone() {
            Some(name) if names.contains(&name) => Some(name),
            _ => names.first().cloned(),
        };

        self.profiles.list = list;
        self.profiles.selected = selected.clone();
        self.profiles.detail = selected
            .as_deref()
            .and_then(|name| self.query(profiles_request("show", kind, Some(name)), cx));
        if self.profiles.agent_target.is_empty() {
            self.profiles.agent_target = "claude".into();
        }

        self.profiles.accounts = self
            .query(
                Request::OrchestratorAccounts {
                    action: "list".into(),
                    name: None,
                    config_dir: None,
                    inherit: None,
                    description: None,
                    default_model: None,
                    default_effort: None,
                },
                cx,
            )
            .and_then(|v| v["accounts"].as_array().cloned())
            .map(|arr| {
                arr.iter()
                    .filter_map(|a| a["name"].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        self.profiles.projects = self
            .query(
                Request::OrchestratorProjects {
                    action: "list".into(),
                    key: None,
                    cwd: None,
                    description: None,
                },
                cx,
            )
            .and_then(|v| v["projects"].as_array().cloned())
            .map(|arr| {
                arr.iter()
                    .filter_map(|p| p["key"].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
    }

    /// プロファイルを更新する dispatch（set）を撃って表示を読み直す。
    /// 名前が無い（＝ 1 つも無い）ときは何もしない
    fn set_profile(&mut self, mutate: impl FnOnce(&mut ProfilesSet), cx: &mut Context<Self>) {
        let Some(name) = self.profiles.selected.clone() else {
            return;
        };
        let mut params = ProfilesSet::default();
        mutate(&mut params);
        self.run(params.into_request(self.profiles.kind, name), cx);
        self.refresh_profiles(cx);
    }

    /// プロファイルタブのテキスト入力の確定（settings_window の commit_edit から呼ばれる）
    pub(super) fn commit_profile_field(
        &mut self,
        field: ProfileField,
        value: String,
        cx: &mut Context<Self>,
    ) {
        match field {
            // 名前は「新規作成」「複製」ボタンで確定するのでバッファに置くだけ
            ProfileField::NewName => self.profiles.new_name = value,
            ProfileField::EnvKey => self.profiles.env_key = value,
            ProfileField::EnvValue => self.profiles.env_value = value,
            ProfileField::Model => self.set_profile(
                |p| {
                    if value.is_empty() {
                        p.clear_model = true;
                    } else {
                        p.model = Some(value);
                    }
                },
                cx,
            ),
            ProfileField::WorkerModel => self.set_profile(
                |p| {
                    if value.is_empty() {
                        p.clear_worker_model = true;
                    } else {
                        p.worker_model = Some(value);
                    }
                },
                cx,
            ),
            // 空文字でクリアする仕様が dispatch 側にあるのでそのまま渡す
            ProfileField::TabNaming => {
                self.set_profile(|p| p.tab_naming_convention = Some(value), cx)
            }
            ProfileField::AgentModel(agent) => self.set_profile(
                |p| {
                    p.agent = Some(agent);
                    if value.is_empty() {
                        p.clear_agent_model = true;
                    } else {
                        p.agent_model = Some(value);
                    }
                },
                cx,
            ),
            ProfileField::AgentArgs(agent) => self.set_profile(
                |p| {
                    p.agent = Some(agent);
                    // 空文字は「引数なし」= 空配列（dispatch が丸ごと置き換える）
                    p.agent_args = Some(
                        value
                            .split_whitespace()
                            .map(str::to_string)
                            .collect::<Vec<_>>(),
                    );
                },
                cx,
            ),
        }
    }

    // --- 描画 ---

    pub(super) fn render_profiles_tab(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let kind = self.profiles.kind;

        div()
            .flex()
            .flex_col()
            .gap_1()
            // 種別（master / solo）
            .child(self.row(
                txt::prof_kind_header(),
                txt::desc_prof_kind(),
                self.kind_switch(cx),
            ))
            .child(self.section(txt::prof_list_header()))
            .child(self.profile_chips(cx))
            .child(self.new_profile_row(cx))
            // 「変更は次回起動から有効」は仕様上の明記事項（受け入れ条件 4）
            .child(
                div()
                    .mt_2()
                    .px_2()
                    .py(px(5.))
                    .rounded(px(5.))
                    .bg(to_hsla(theme.surface_1))
                    .text_color(to_hsla(theme.text_secondary))
                    .text_size(px(11.))
                    .child(txt::prof_restart_note()),
            )
            .children(self.profiles.selected.as_ref().map(|name| {
                let launch = orchestrator::launch_command(kind.launch_bin(), name);
                div()
                    .mt_1()
                    .text_color(to_hsla(theme.text_muted))
                    .text_size(px(11.))
                    .child(format!("{}: {launch}", txt::prof_launch_label()))
            }))
            .children(self.profile_detail(cx))
    }

    /// master / solo の切替
    fn kind_switch(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let current = self.profiles.kind;
        let mut row = div()
            .flex()
            .gap(px(2.))
            .p(px(2.))
            .rounded(px(7.))
            .bg(to_hsla(theme.surface_1));
        for kind in [ProfileKind::Master, ProfileKind::Solo] {
            let active = kind == current;
            row = row.child(
                div()
                    .id(SharedString::from(format!("prof-kind-{}", kind.as_str())))
                    .px_3()
                    .py(px(4.))
                    .rounded(px(5.))
                    .bg(if active {
                        to_hsla(theme.accent)
                    } else {
                        transparent_black()
                    })
                    .text_color(if active {
                        gpui::white()
                    } else {
                        to_hsla(theme.text_muted)
                    })
                    .text_size(px(12.))
                    .cursor_pointer()
                    .when(!active, |d| {
                        d.hover(|s| s.bg(to_hsla(theme.surface_hover_strong)))
                    })
                    .child(kind_label(kind))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // 種別ごとに選択を独立させる（切替時は先頭を選び直す）
                        this.profiles.kind = kind;
                        this.profiles.selected = None;
                        this.profiles.confirm_delete = None;
                        this.message = None;
                        this.refresh_profiles(cx);
                        cx.notify();
                    })),
            );
        }
        row
    }

    /// プロファイル一覧（選択チップ）
    fn profile_chips(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        if self.profiles.list.is_empty() {
            return div()
                .py(px(4.))
                .text_color(to_hsla(theme.text_muted))
                .text_size(px(12.))
                .child(txt::prof_empty());
        }
        let selected = self.profiles.selected.clone();
        let mut row = div().flex().flex_wrap().gap(px(4.)).py(px(2.));
        for profile in &self.profiles.list {
            let Some(name) = profile["name"].as_str().map(String::from) else {
                continue;
            };
            let active = selected.as_deref() == Some(name.as_str());
            // 壊れた yaml は赤系で示す（隠さない = 直せるようにする）
            let broken = profile.get("error").is_some();
            let has_warning = profile.get("warnings").is_some();
            let for_click = name.clone();
            row = row.child(
                div()
                    .id(SharedString::from(format!("prof-chip-{name}")))
                    .px_3()
                    .py(px(4.))
                    .rounded(px(6.))
                    .bg(if active {
                        to_hsla(theme.accent)
                    } else if broken {
                        to_hsla(theme.danger_surface)
                    } else {
                        to_hsla(theme.chip_surface)
                    })
                    .text_color(if active {
                        gpui::white()
                    } else if broken {
                        to_hsla(theme.red)
                    } else {
                        to_hsla(theme.foreground)
                    })
                    .text_size(px(12.))
                    .cursor_pointer()
                    .when(!active, |d| d.hover(|s| s.bg(to_hsla(theme.surface_hover))))
                    .child(if has_warning && !broken {
                        // 記号は絵文字を使わない（UI 絵文字ゼロの原則）
                        format!("{name} *")
                    } else {
                        name.clone()
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.profiles.selected = Some(for_click.clone());
                        this.profiles.confirm_delete = None;
                        this.message = None;
                        this.refresh_profiles(cx);
                        cx.notify();
                    })),
            );
        }
        row
    }

    /// 新規作成 / 複製の行
    fn new_profile_row(&self, cx: &mut Context<Self>) -> Div {
        let has_selection = self.profiles.selected.is_some();
        div()
            .flex()
            .items_center()
            .gap_2()
            .py(px(4.))
            .child(self.text_field(
                EditField::Profile(ProfileField::NewName),
                &self.profiles.new_name,
                txt::prof_new_placeholder(),
                None,
                cx,
            ))
            .child(self.button(
                "prof-create",
                txt::prof_create(),
                BtnKind::Primary,
                cx.listener(|this, _, _, cx| this.create_profile(None, cx)),
            ))
            .child(self.button(
                "prof-duplicate",
                txt::prof_duplicate(),
                if has_selection {
                    BtnKind::Normal
                } else {
                    BtnKind::Disabled
                },
                cx.listener(|this, _, _, cx| {
                    let from = this.profiles.selected.clone();
                    this.create_profile(from, cx);
                }),
            ))
    }

    /// 新規作成（from = None）/ 複製（from = Some(複製元)）
    fn create_profile(&mut self, from: Option<String>, cx: &mut Context<Self>) {
        // 入力途中（Enter 未押下）でもボタンで確定できるようにバッファを取り込む
        if let Some(edit) = self.editing(&EditField::Profile(ProfileField::NewName)) {
            self.profiles.new_name = edit.text.trim().to_string();
            self.edit = None;
        }
        let name = self.profiles.new_name.trim().to_string();
        if name.is_empty() {
            self.message = Some((txt::prof_name_required().to_string(), true));
            cx.notify();
            return;
        }
        let kind = self.profiles.kind;
        let request = Request::OrchestratorProfiles {
            action: if from.is_some() { "copy" } else { "create" }.into(),
            name: Some(name.clone()),
            kind: Some(kind.as_str().into()),
            from,
            projects: None,
            clear_projects: false,
            master_agent: None,
            clear_master_agent: false,
            model: None,
            worker_model: None,
            effort: None,
            worker_effort: None,
            clear_model: false,
            clear_worker_model: false,
            worker_agent: None,
            clear_worker_agent: false,
            agent: None,
            agent_model: None,
            clear_agent_model: false,
            agent_effort: None,
            clear_agent_effort: false,
            agent_skip_permissions: None,
            agent_args: None,
            worker_model_policy: None,
            tab_naming_convention: None,
            env_set: None,
            env_unset: None,
            master_account: None,
            clear_master_account: false,
            worker_account: None,
            clear_worker_account: false,
        };
        match self.dispatch(request, cx) {
            Ok(_) => {
                self.message = None;
                self.profiles.new_name.clear();
                self.profiles.selected = Some(name);
            }
            Err(e) => self.message = Some((e, true)),
        }
        self.refresh_profiles(cx);
        cx.notify();
    }

    /// 選択中プロファイルの詳細フォーム
    fn profile_detail(&self, cx: &mut Context<Self>) -> Option<Div> {
        let theme = self.theme();
        let detail = self.profiles.detail.clone()?;
        let name = self.profiles.selected.clone()?;

        // パースできない yaml は編集させない（default に丸めた上書きで設定を消さない）
        if let Some(error) = detail.get("error").and_then(|e| e.as_str()) {
            return Some(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .mt_3()
                    .child(self.notice(txt::prof_broken(), true))
                    .child(
                        div()
                            .text_color(to_hsla(theme.text_muted))
                            .text_size(px(11.))
                            .child(error.to_string()),
                    ),
            );
        }

        let master_agent = detail["master_agent"].as_str().unwrap_or("").to_string();
        let worker_agent = detail["worker_agent"].as_str().unwrap_or("").to_string();
        let policy = detail["worker_model_policy"]
            .as_str()
            .unwrap_or("inherit")
            .to_string();

        Some(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .mt_2()
                .children(self.profile_warnings(&detail))
                // --- master ---
                .child(self.section(txt::prof_section_master()))
                .child(self.row(
                    txt::prof_label_agent(),
                    txt::desc_prof_master_agent(),
                    self.option_chips(
                        "prof-master-agent",
                        &[
                            (String::new(), txt::prof_option_default().to_string()),
                            ("claude".into(), "claude".into()),
                            ("codex".into(), "codex".into()),
                        ],
                        &master_agent,
                        cx,
                        |this, value, cx| {
                            this.set_profile(
                                |p| {
                                    if value.is_empty() {
                                        p.clear_master_agent = true;
                                    } else {
                                        p.master_agent = Some(value);
                                    }
                                },
                                cx,
                            );
                        },
                    ),
                ))
                .child(self.model_row(
                    txt::prof_label_model(),
                    txt::desc_prof_model(),
                    EditField::Profile(ProfileField::Model),
                    detail["model"].as_str().unwrap_or(""),
                    cx,
                ))
                .child(self.effort_row(
                    txt::prof_label_effort(),
                    &master_agent,
                    detail["effort"].as_str().unwrap_or(""),
                    "prof-effort",
                    cx,
                    |this, value, cx| this.set_profile(|p| p.effort = Some(value), cx),
                ))
                .child(self.account_row(
                    txt::prof_label_master_account(),
                    detail["master_account"].as_str().unwrap_or(""),
                    "prof-master-account",
                    cx,
                    |this, value, cx| {
                        this.set_profile(
                            |p| {
                                if value.is_empty() {
                                    p.clear_master_account = true;
                                } else {
                                    p.master_account = Some(value);
                                }
                            },
                            cx,
                        );
                    },
                ))
                // --- worker ---
                .child(self.section(txt::prof_section_worker()))
                .child(self.row(
                    txt::prof_label_agent(),
                    txt::desc_prof_worker_agent(),
                    self.option_chips(
                        "prof-worker-agent",
                        &[
                            (String::new(), txt::prof_option_default().to_string()),
                            ("claude".into(), "claude".into()),
                            ("codex".into(), "codex".into()),
                            ("agy".into(), "agy".into()),
                        ],
                        &worker_agent,
                        cx,
                        |this, value, cx| {
                            this.set_profile(
                                |p| {
                                    if value.is_empty() {
                                        p.clear_worker_agent = true;
                                    } else {
                                        p.worker_agent = Some(value);
                                    }
                                },
                                cx,
                            );
                        },
                    ),
                ))
                .child(self.row(
                    txt::prof_label_policy(),
                    "",
                    self.option_chips(
                        "prof-policy",
                        &[
                            ("inherit".into(), txt::prof_policy_inherit().to_string()),
                            ("delegate".into(), txt::prof_policy_delegate().to_string()),
                            ("fixed".into(), txt::prof_policy_fixed().to_string()),
                        ],
                        &policy,
                        cx,
                        |this, value, cx| {
                            this.set_profile(|p| p.worker_model_policy = Some(value), cx)
                        },
                    ),
                ))
                .child(self.model_row(
                    txt::prof_label_worker_model(),
                    txt::desc_prof_worker_model(),
                    EditField::Profile(ProfileField::WorkerModel),
                    detail["worker_model"].as_str().unwrap_or(""),
                    cx,
                ))
                .child(self.effort_row(
                    txt::prof_label_worker_effort(),
                    &worker_agent,
                    detail["worker_effort"].as_str().unwrap_or(""),
                    "prof-worker-effort",
                    cx,
                    |this, value, cx| this.set_profile(|p| p.worker_effort = Some(value), cx),
                ))
                .child(self.account_row(
                    txt::prof_label_worker_account(),
                    detail["worker_account"].as_str().unwrap_or(""),
                    "prof-worker-account",
                    cx,
                    |this, value, cx| {
                        this.set_profile(
                            |p| {
                                if value.is_empty() {
                                    p.clear_worker_account = true;
                                } else {
                                    p.worker_account = Some(value);
                                }
                            },
                            cx,
                        );
                    },
                ))
                // --- エージェント別設定 ---
                .child(self.section(txt::prof_section_agent()))
                .child(self.agent_specific(&detail, cx))
                // --- プロジェクト ---
                .child(self.section(txt::prof_section_projects()))
                .child(self.projects_picker(&detail, cx))
                // --- 環境変数 ---
                .child(self.section(txt::prof_section_env()))
                .child(self.env_editor(&detail, cx))
                // --- その他 ---
                .child(self.section(txt::prof_section_other()))
                .child(self.row(
                    txt::prof_label_tab_naming(),
                    txt::desc_prof_tab_naming(),
                    self.text_field(
                        EditField::Profile(ProfileField::TabNaming),
                        detail["tab_naming_convention"].as_str().unwrap_or(""),
                        "",
                        Some(px(260.)),
                        cx,
                    ),
                ))
                .child(self.delete_row(&name, cx))
                .children(detail["path"].as_str().map(|path| {
                    div()
                        .mt_1()
                        .text_color(to_hsla(theme.text_faint))
                        .text_size(px(10.))
                        .child(format!("{}: {path}", txt::prof_path_label()))
                })),
        )
    }

    /// dispatch が返した参照整合性の警告（未登録 project / アカウント / [1m] モデル）。
    /// 保存前の確認材料として詳細フォームの先頭に出す
    fn profile_warnings(&self, detail: &serde_json::Value) -> Option<Div> {
        let theme = self.theme();
        let warnings = detail["warnings"].as_array()?;
        if warnings.is_empty() {
            return None;
        }
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .mb_2()
                .px_2()
                .py(px(6.))
                .rounded(px(5.))
                .bg(to_hsla(theme.danger_surface))
                .child(
                    div()
                        .text_color(to_hsla(theme.red))
                        .text_size(px(11.))
                        .child(txt::prof_warnings_header()),
                )
                .children(warnings.iter().filter_map(|w| {
                    w.as_str().map(|text| {
                        div()
                            .text_color(to_hsla(theme.text_secondary))
                            .text_size(px(11.))
                            .child(text.to_string())
                    })
                })),
        )
    }

    /// エージェント別の worker 設定（対象を選んでモデル / effort / 承認 / 引数）
    fn agent_specific(&self, detail: &serde_json::Value, cx: &mut Context<Self>) -> Div {
        let target = self.profiles.agent_target.clone();
        let cfg = detail
            .get("worker_agents")
            .and_then(|v| v.get(&target))
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let model = cfg["model"].as_str().unwrap_or("").to_string();
        let effort = cfg["effort"].as_str().unwrap_or("").to_string();
        // 未設定時は種別の既定（判定は tako-control 側の 1 実装から引く。UI で二重定義しない）
        let skip = cfg["skip_permissions"].as_bool().unwrap_or_else(|| {
            WorkerAgent::parse(&target)
                .map(|a| a.default_skip_permissions())
                .unwrap_or(false)
        });
        let args = cfg["args"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        let target_for_model = target.clone();
        let target_for_args = target.clone();
        let target_for_skip = target.clone();
        let target_for_effort = target.clone();

        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(self.row(
                txt::prof_agent_target(),
                txt::desc_prof_agent_target(),
                self.option_chips(
                    "prof-agent-target",
                    &[
                        ("claude".into(), "claude".into()),
                        ("codex".into(), "codex".into()),
                        ("agy".into(), "agy".into()),
                    ],
                    &target,
                    cx,
                    |this, value, cx| {
                        this.profiles.agent_target = value;
                        cx.notify();
                    },
                ),
            ))
            .child(self.model_row(
                txt::prof_label_model(),
                txt::desc_prof_model(),
                EditField::Profile(ProfileField::AgentModel(target_for_model)),
                &model,
                cx,
            ))
            .child(self.effort_row(
                txt::prof_label_effort(),
                &target_for_effort,
                &effort,
                "prof-agent-effort",
                cx,
                move |this, value, cx| {
                    let agent = this.profiles.agent_target.clone();
                    this.set_profile(
                        |p| {
                            p.agent = Some(agent);
                            if value.is_empty() {
                                p.clear_agent_effort = true;
                            } else {
                                p.agent_effort = Some(value);
                            }
                        },
                        cx,
                    );
                },
            ))
            .child(self.row(
                txt::prof_label_skip_permissions(),
                txt::desc_prof_skip_permissions(),
                self.toggle(
                    "prof-skip-permissions",
                    skip,
                    cx.listener(move |this, _, _, cx| {
                        let agent = target_for_skip.clone();
                        this.set_profile(
                            |p| {
                                p.agent = Some(agent);
                                p.agent_skip_permissions = Some(!skip);
                            },
                            cx,
                        );
                    }),
                ),
            ))
            .child(self.row(
                txt::prof_label_agent_args(),
                txt::desc_prof_agent_args(),
                self.text_field(
                    EditField::Profile(ProfileField::AgentArgs(target_for_args)),
                    &args,
                    "",
                    Some(px(220.)),
                    cx,
                ),
            ))
    }

    /// 担当プロジェクトの複数選択。**登録済みキーしか選べない**ので
    /// GUI から未登録の参照を作れない（受け入れ条件 3 の構造的な担保）
    fn projects_picker(&self, detail: &serde_json::Value, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let assigned: Vec<String> = detail["projects"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if self.profiles.projects.is_empty() && assigned.is_empty() {
            return div()
                .py(px(4.))
                .text_color(to_hsla(theme.text_muted))
                .text_size(px(11.))
                .child(txt::prof_no_projects());
        }
        // 未登録だが割り当て済みのキーも並べる（外して直せるようにする）
        let mut keys = self.profiles.projects.clone();
        for key in &assigned {
            if !keys.contains(key) {
                keys.push(key.clone());
            }
        }
        let mut row = div().flex().flex_wrap().gap(px(4.)).py(px(2.));
        for key in keys {
            let active = assigned.contains(&key);
            let known = self.profiles.projects.contains(&key);
            let mut next = assigned.clone();
            if active {
                next.retain(|k| k != &key);
            } else {
                next.push(key.clone());
            }
            row = row.child(
                div()
                    .id(SharedString::from(format!("prof-project-{key}")))
                    .px_3()
                    .py(px(4.))
                    .rounded(px(6.))
                    .bg(if active {
                        to_hsla(theme.accent)
                    } else {
                        to_hsla(theme.chip_surface)
                    })
                    .text_color(if active && known {
                        gpui::white()
                    } else if !known {
                        to_hsla(theme.red)
                    } else {
                        to_hsla(theme.foreground)
                    })
                    .text_size(px(12.))
                    .cursor_pointer()
                    .when(!active, |d| d.hover(|s| s.bg(to_hsla(theme.surface_hover))))
                    .child(key.clone())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let next = next.clone();
                        this.set_profile(
                            |p| {
                                if next.is_empty() {
                                    p.clear_projects = true;
                                } else {
                                    p.projects = Some(next);
                                }
                            },
                            cx,
                        );
                    })),
            );
        }
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .text_color(to_hsla(theme.text_muted))
                    .text_size(px(11.))
                    .child(txt::desc_prof_projects()),
            )
            .child(row)
    }

    /// 環境変数の一覧（値はマスク）と追加・削除。
    /// 値は dispatch の応答でも `***` になっているので UI に生値は入って来ない（#500）
    fn env_editor(&self, detail: &serde_json::Value, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let keys: Vec<String> = detail["env"]
            .as_object()
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default();
        div()
            .flex()
            .flex_col()
            .gap(px(3.))
            .child(
                div()
                    .text_color(to_hsla(theme.text_muted))
                    .text_size(px(11.))
                    .child(txt::desc_prof_env()),
            )
            .children(keys.into_iter().map(|key| {
                let for_click = key.clone();
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .py(px(2.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_color(to_hsla(theme.foreground))
                            .text_size(px(12.))
                            .child(format!("{key} {}", txt::prof_env_masked())),
                    )
                    .child(self.button(
                        &format!("prof-env-del-{key}"),
                        txt::button_delete(),
                        BtnKind::Danger,
                        cx.listener(move |this, _, _, cx| {
                            let key = for_click.clone();
                            this.set_profile(|p| p.env_unset = Some(vec![key]), cx);
                        }),
                    ))
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .py(px(2.))
                    .child(self.text_field(
                        EditField::Profile(ProfileField::EnvKey),
                        &self.profiles.env_key,
                        "KEY",
                        Some(px(150.)),
                        cx,
                    ))
                    .child(self.text_field(
                        EditField::Profile(ProfileField::EnvValue),
                        &self.profiles.env_value,
                        "VALUE",
                        None,
                        cx,
                    ))
                    .child(self.button(
                        "prof-env-add",
                        txt::prof_env_add(),
                        BtnKind::Normal,
                        cx.listener(|this, _, _, cx| this.add_env(cx)),
                    )),
            )
    }

    /// 環境変数の追加（KEY / VALUE の入力途中でもボタンで確定できる）
    fn add_env(&mut self, cx: &mut Context<Self>) {
        for field in [ProfileField::EnvKey, ProfileField::EnvValue] {
            if let Some(edit) = self.editing(&EditField::Profile(field.clone())) {
                let text = edit.text.trim().to_string();
                match field {
                    ProfileField::EnvKey => self.profiles.env_key = text,
                    _ => self.profiles.env_value = text,
                }
                self.edit = None;
            }
        }
        let key = self.profiles.env_key.trim().to_string();
        if key.is_empty() {
            return;
        }
        let entry = format!("{key}={}", self.profiles.env_value);
        self.set_profile(|p| p.env_set = Some(vec![entry]), cx);
        // 成功時のみ入力欄を空にする（失敗したら打ち直しにならないよう残す）
        if self.message.is_none() {
            self.profiles.env_key.clear();
            self.profiles.env_value.clear();
        }
        cx.notify();
    }

    /// 削除（確認つき）
    fn delete_row(&self, name: &str, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let confirming = self.profiles.confirm_delete.as_deref() == Some(name);
        let name_owned = name.to_string();
        if !confirming {
            return div().flex().justify_end().pt_3().child(self.button(
                "prof-delete",
                txt::button_delete(),
                BtnKind::Danger,
                cx.listener(move |this, _, _, cx| {
                    this.profiles.confirm_delete = Some(name_owned.clone());
                    cx.notify();
                }),
            ));
        }
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .mt_3()
            .px_2()
            .py(px(6.))
            .rounded(px(5.))
            .bg(to_hsla(theme.danger_surface))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_color(to_hsla(theme.red))
                    .text_size(px(12.))
                    .child(format!("{name}: {}", txt::prof_delete_confirm())),
            )
            .child(self.button(
                "prof-delete-cancel",
                txt::prof_cancel(),
                BtnKind::Normal,
                cx.listener(|this, _, _, cx| {
                    this.profiles.confirm_delete = None;
                    cx.notify();
                }),
            ))
            .child(self.button(
                "prof-delete-yes",
                txt::button_delete(),
                BtnKind::Danger,
                cx.listener(move |this, _, _, cx| this.delete_profile(cx)),
            ))
    }

    fn delete_profile(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.profiles.confirm_delete.take() else {
            return;
        };
        let kind = self.profiles.kind;
        match self.dispatch(profiles_request("delete", kind, Some(&name)), cx) {
            Ok(_) => {
                self.message = None;
                self.profiles.selected = None;
            }
            Err(e) => self.message = Some((e, true)),
        }
        self.refresh_profiles(cx);
        cx.notify();
    }

    // --- 小さな共通パーツ ---

    /// 単一選択チップ。`values` は (保存値, 表示ラベル)
    fn option_chips(
        &self,
        id: &str,
        values: &[(String, String)],
        current: &str,
        cx: &mut Context<Self>,
        on_pick: impl Fn(&mut Self, String, &mut Context<Self>) + Clone + 'static,
    ) -> Div {
        let theme = self.theme();
        let mut row = div().flex().flex_wrap().gap(px(3.)).justify_end();
        for (value, label) in values {
            let active = value == current;
            let value_owned = value.clone();
            let pick = on_pick.clone();
            row = row.child(
                div()
                    .id(SharedString::from(format!("{id}-{value}")))
                    .px_2()
                    .py(px(4.))
                    .rounded(px(5.))
                    .bg(if active {
                        to_hsla(theme.accent)
                    } else {
                        to_hsla(theme.surface_1)
                    })
                    .text_color(if active {
                        gpui::white()
                    } else {
                        to_hsla(theme.text_muted)
                    })
                    .text_size(px(12.))
                    .cursor_pointer()
                    .when(!active, |d| {
                        d.hover(|s| s.bg(to_hsla(theme.surface_hover_strong)))
                    })
                    .child(label.clone())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        pick(this, value_owned.clone(), cx);
                    })),
            );
        }
        row
    }

    /// モデル行（自由入力 + [1m] の保存前警告）。
    /// 選択肢を持てない（上流 CLI のリリースでモデル名が変わる）ため入力欄にしている
    fn model_row(
        &self,
        label: &str,
        desc: &str,
        field: EditField,
        current: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = self.theme();
        // 入力中のバッファを見るので、Enter を押す前に警告が出る（受け入れ条件 3）
        let pending = self
            .editing(&field)
            .map(|e| e.text.clone())
            .unwrap_or_else(|| current.to_string());
        let warn_1m = pending.contains("[1m]");
        div()
            .flex()
            .flex_col()
            .child(self.row(
                label,
                desc,
                self.text_field(
                    field,
                    current,
                    txt::prof_option_default(),
                    Some(px(220.)),
                    cx,
                ),
            ))
            .when(warn_1m, |d| {
                d.child(
                    div()
                        .pb(px(4.))
                        .text_color(to_hsla(theme.red))
                        .text_size(px(11.))
                        .child(txt::prof_model_1m_warning()),
                )
            })
    }

    /// effort 行。選択肢はエージェント種別ごとの既知の値（agy は指定手段なし）
    fn effort_row(
        &self,
        label: &str,
        agent: &str,
        current: &str,
        id: &str,
        cx: &mut Context<Self>,
        on_pick: impl Fn(&mut Self, String, &mut Context<Self>) + Clone + 'static,
    ) -> Div {
        let theme = self.theme();
        let agent = WorkerAgent::parse(agent).unwrap_or(WorkerAgent::Claude);
        let options = agent.effort_options();
        if options.is_empty() {
            return self.row(
                label,
                "",
                div()
                    .text_color(to_hsla(theme.text_muted))
                    .text_size(px(11.))
                    .child(txt::prof_effort_ignored()),
            );
        }
        let mut values: Vec<(String, String)> =
            vec![(String::new(), txt::prof_option_unset().to_string())];
        values.extend(options.iter().map(|v| (v.to_string(), v.to_string())));
        // 上流 CLI が語彙を増やした場合でも現在値を選択肢として見せる（消さない）
        if !current.is_empty() && !options.contains(&current) {
            values.push((current.to_string(), current.to_string()));
        }
        self.row(
            label,
            "",
            self.option_chips(id, &values, current, cx, on_pick),
        )
    }

    /// アカウント行。**登録済みアカウントしか選べない**（未登録参照を GUI から作れない）
    fn account_row(
        &self,
        label: &str,
        current: &str,
        id: &str,
        cx: &mut Context<Self>,
        on_pick: impl Fn(&mut Self, String, &mut Context<Self>) + Clone + 'static,
    ) -> Div {
        let theme = self.theme();
        if self.profiles.accounts.is_empty() && current.is_empty() {
            return self.row(
                label,
                "",
                div()
                    .text_color(to_hsla(theme.text_muted))
                    .text_size(px(11.))
                    .child(txt::prof_no_accounts()),
            );
        }
        let mut values: Vec<(String, String)> =
            vec![(String::new(), txt::prof_option_unset().to_string())];
        for account in &self.profiles.accounts {
            values.push((account.clone(), account.clone()));
        }
        // 未登録だが設定済みの名前も出す（外して直せるようにする）
        if !current.is_empty() && !self.profiles.accounts.iter().any(|a| a == current) {
            values.push((current.to_string(), current.to_string()));
        }
        self.row(
            label,
            txt::desc_prof_account(),
            self.option_chips(id, &values, current, cx, on_pick),
        )
    }

    /// 注意書きの帯
    fn notice(&self, text: &str, is_error: bool) -> Div {
        let theme = self.theme();
        div()
            .px_2()
            .py(px(6.))
            .rounded(px(5.))
            .bg(if is_error {
                to_hsla(theme.danger_surface)
            } else {
                to_hsla(theme.surface_1)
            })
            .text_color(if is_error {
                to_hsla(theme.red)
            } else {
                to_hsla(theme.text_secondary)
            })
            .text_size(px(11.))
            .child(text.to_string())
    }
}

// --- セルフテスト用の入口（項目 94）---
//
// プロファイルタブは見た目ではなく**状態遷移**が壊れると困る（別プロファイルを
// 保存する / 確認なしで消える / 外部変更に追随しない）。ピクセルを見なくても
// 検証できるよう、ボタン・入力欄が通るのと同じ内部経路をここから叩けるようにする。
// UI の描画そのものは実機の目視（`.agent/manual-checks.md`）に委ねる
impl SettingsWindow {
    /// 種別を切り替える（種別スイッチのクリックと同じ経路）
    pub(crate) fn st_profiles_set_kind(&mut self, kind: ProfileKind, cx: &mut Context<Self>) {
        self.profiles.kind = kind;
        self.profiles.selected = None;
        self.refresh_profiles(cx);
    }

    /// プロファイルを選ぶ（一覧チップのクリックと同じ経路）
    pub(crate) fn st_profiles_select(&mut self, name: &str, cx: &mut Context<Self>) {
        self.profiles.selected = Some(name.to_string());
        self.refresh_profiles(cx);
    }

    /// 一覧の表示名（描画に使っているのと同じ配列から取る）
    pub(crate) fn st_profiles_names(&self) -> Vec<String> {
        self.profiles
            .list
            .iter()
            .filter_map(|p| p["name"].as_str().map(String::from))
            .collect()
    }

    /// 選択中プロファイルの詳細（フォームが表示に使っている生 JSON）
    pub(crate) fn st_profiles_detail(&self) -> Option<serde_json::Value> {
        self.profiles.detail.clone()
    }

    /// 入力欄の確定（Enter と同じ経路 = OrchestratorProfiles set が飛ぶ）
    pub(crate) fn st_profiles_commit(
        &mut self,
        field: ProfileField,
        value: &str,
        cx: &mut Context<Self>,
    ) {
        self.commit_profile_field(field, value.to_string(), cx);
    }

    /// 単一選択チップの押下（保存値を直接渡す）
    pub(crate) fn st_profiles_pick_effort(&mut self, value: &str, cx: &mut Context<Self>) {
        let value = value.to_string();
        self.set_profile(|p| p.effort = Some(value), cx);
    }

    /// 削除ボタン 1 回目（確認待ちにするだけ。まだ消さない）
    pub(crate) fn st_profiles_request_delete(&mut self, name: &str) {
        self.profiles.confirm_delete = Some(name.to_string());
    }

    /// 削除ボタン 2 回目（確認を経て実際に消す）
    pub(crate) fn st_profiles_confirm_delete(&mut self, cx: &mut Context<Self>) {
        self.delete_profile(cx);
    }
}

/// list / show / delete 用の最小リクエスト
fn profiles_request(action: &str, kind: ProfileKind, name: Option<&str>) -> Request {
    Request::OrchestratorProfiles {
        action: action.into(),
        name: name.map(String::from),
        kind: Some(kind.as_str().into()),
        from: None,
        projects: None,
        clear_projects: false,
        master_agent: None,
        clear_master_agent: false,
        model: None,
        worker_model: None,
        effort: None,
        worker_effort: None,
        clear_model: false,
        clear_worker_model: false,
        worker_agent: None,
        clear_worker_agent: false,
        agent: None,
        agent_model: None,
        clear_agent_model: false,
        agent_effort: None,
        clear_agent_effort: false,
        agent_skip_permissions: None,
        agent_args: None,
        worker_model_policy: None,
        tab_naming_convention: None,
        env_set: None,
        env_unset: None,
        master_account: None,
        clear_master_account: false,
        worker_account: None,
        clear_worker_account: false,
    }
}

/// `set` のパラメータ。項目が多いので Request の全フィールドを毎回書かずに済むよう
/// 既定値つきの箱にしてある（中身は Request と 1:1）
#[derive(Default)]
struct ProfilesSet {
    master_agent: Option<String>,
    clear_master_agent: bool,
    model: Option<String>,
    clear_model: bool,
    worker_model: Option<String>,
    clear_worker_model: bool,
    effort: Option<String>,
    worker_effort: Option<String>,
    worker_agent: Option<String>,
    clear_worker_agent: bool,
    agent: Option<String>,
    agent_model: Option<String>,
    clear_agent_model: bool,
    agent_effort: Option<String>,
    clear_agent_effort: bool,
    agent_skip_permissions: Option<bool>,
    agent_args: Option<Vec<String>>,
    worker_model_policy: Option<String>,
    tab_naming_convention: Option<String>,
    env_set: Option<Vec<String>>,
    env_unset: Option<Vec<String>>,
    master_account: Option<String>,
    clear_master_account: bool,
    worker_account: Option<String>,
    clear_worker_account: bool,
    projects: Option<Vec<String>>,
    clear_projects: bool,
}

impl ProfilesSet {
    fn into_request(self, kind: ProfileKind, name: String) -> Request {
        Request::OrchestratorProfiles {
            action: "set".into(),
            name: Some(name),
            kind: Some(kind.as_str().into()),
            from: None,
            projects: self.projects,
            clear_projects: self.clear_projects,
            master_agent: self.master_agent,
            clear_master_agent: self.clear_master_agent,
            model: self.model,
            worker_model: self.worker_model,
            effort: self.effort,
            worker_effort: self.worker_effort,
            clear_model: self.clear_model,
            clear_worker_model: self.clear_worker_model,
            worker_agent: self.worker_agent,
            clear_worker_agent: self.clear_worker_agent,
            agent: self.agent,
            agent_model: self.agent_model,
            clear_agent_model: self.clear_agent_model,
            agent_effort: self.agent_effort,
            clear_agent_effort: self.clear_agent_effort,
            agent_skip_permissions: self.agent_skip_permissions,
            agent_args: self.agent_args,
            worker_model_policy: self.worker_model_policy,
            tab_naming_convention: self.tab_naming_convention,
            env_set: self.env_set,
            env_unset: self.env_unset,
            master_account: self.master_account,
            clear_master_account: self.clear_master_account,
            worker_account: self.worker_account,
            clear_worker_account: self.clear_worker_account,
        }
    }
}
