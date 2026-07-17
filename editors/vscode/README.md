# clum の vscode シンタックスハイライト（粗版）

自前 lexer を正とする方針（docs/plan/002.md トラック C-2 案 (b)）の TextMate 粗版。
行頭トークンの字句クラスに基づく文法であり、名前解決を要する区別
（子ブロック内のテキスト行とコード行、属性名の検査など）は行わない。
精密化は LSP セマンティックトークン（トラック C-3）で行う。

## インストール

vsix を作らず、拡張ディレクトリへのシンボリックリンクで使う:

```bash
ln -s ~/h/clum/editors/vscode ~/.vscode/extensions/clum.clum-0.0.1
```

リンク後に vscode を再起動（またはウィンドウの再読み込み）する。

## 含まれるもの

- `package.json` — 言語定義（`*.clum`）と文法の登録
- `syntaxes/clum.tmLanguage.json` — TextMate 文法
- `language-configuration.json` — `//` コメント・対応括弧・kebab-case の単語境界・
  インデントベースの折りたたみ（offSide）

## ハイライトの対応と割り切り

nvim 版（`../nvim/README.md`）と同一。子ブロック内のテキスト行と属性名は無色、
行頭 UpperCamelCase のテキストは定義名と同色（仕様上もコンポーネント参照と
解釈される位置のため、挙動としては仕様に忠実）。
