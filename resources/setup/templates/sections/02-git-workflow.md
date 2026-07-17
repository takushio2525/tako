# Git 運用

<!-- coverage: コミット規約 = コミット | commit -->
<!-- coverage: push・ブランチ運用 = push | ブランチ | branch | マージ | merge | pull request -->

## 必須概念

- コミットメッセージのフォーマットを定義する
- コミットの粒度（機能単位か一括か）を定義する
- push の運用ルールを定義する

## 参考テンプレート

### シンプル（個人開発）

```markdown
## Git コミット

- 作業完了時にコミットする
- メッセージ: 変更内容を簡潔に記述する
- 機能単位で分割（無関係な変更を 1 コミットにまとめない）
```

### チーム開発

```markdown
## Git 運用

- コミットメッセージ: `[種別] 変更内容の概要`
  - 種別: feat / fix / refactor / docs / style / test
- 機能単位で細かく分割する
- 影響が大きい変更はブランチを切って PR 経由でマージする
- main への直接 push は軽微な修正のみ
```

### 厳格（チーム・OSS）

```markdown
## Git 運用

- Issue ファーストで作業する（Issue → Branch → PR → squash merge）
- ブランチ名: `feat/123-説明` / `fix/123-説明`
- コミットメッセージに Issue 番号を含める
- main への直接 push は禁止（すべて PR 経由）
- マージ済みブランチは即削除
```
