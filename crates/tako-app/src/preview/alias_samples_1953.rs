//! #1953 の検査用の本文（単体 `syntax_resolution_tests` と visual-test `syntax-alias` が同じものを開く）。
//! 授業・設定でよく見る形にし、本文の判定（`tako_core::file_type::syntax_by_content`）が見る手掛かり
//! （`%` / `function` / `#import` / `@interface`）と、JSON5 にしか無い書き方を入れてある

/// C / C++ のヘッダ（`.h` = C++ で塗る）
pub(crate) const C_HEADER: &str = "\
#ifndef GEO_H
#define GEO_H
#include <stdint.h>

namespace geo {
template <typename T>
struct Point {
    T x, y; // 座標
    constexpr Point(T x, T y) : x(x), y(y) {}
};
}

int32_t add(int32_t a, int32_t b);
#endif
";

/// Objective-C のヘッダ（`.h` でも本文の手掛かりで Objective-C のまま）
pub(crate) const OBJC_HEADER: &str = "\
#import <Foundation/Foundation.h>

@interface Counter : NSObject
@property (nonatomic) NSInteger value;
- (void)increment;
@end
";

/// MATLAB の関数ファイル（`.m` = 本文の手掛かりで MATLAB）
pub(crate) const MATLAB_SOURCE: &str = "\
% 二次方程式 a x^2 + b x + c = 0 を解く
function x = solve(a, b, c)
    d = b^2 - 4*a*c;   % 判別式
    if d < 0
        error('実数解なし');
    end
    x = (-b + [1 -1] * sqrt(d)) / (2*a);
end
";

/// Objective-C の実装（`.m` = 本文の手掛かりで Objective-C）
pub(crate) const OBJC_SOURCE: &str = "\
#import \"Counter.h\"

@implementation Counter
- (void)increment {
    self.value += 1;
}
@end
";

/// Processing のスケッチ（`.pde` = Java で塗る）
pub(crate) const PROCESSING_SKETCH: &str = "\
int x = 0;

void setup() {
  size(400, 400);
}

void draw() {
  background(0);
  ellipse(mouseX, mouseY, 20, 20); // マウスを追う
  x += 1;
}
";

/// JSONC（`.jsonc` = JSON で塗る。コメントと末尾カンマ）
pub(crate) const JSONC_SETTINGS: &str = "\
{
  // VS Code の設定
  \"editor.tabSize\": 2,
  /* 末尾カンマ */
  \"files.exclude\": { \"**/.git\": true, },
}
";

/// JSON5（`.json5` = JavaScript で塗る）。クォートの無いキー・`'…'`・16 進・`+Infinity`・`.5`・
/// コメント・末尾カンマ = JSON の構文なら `invalid` になる書き方を全部入れてある
pub(crate) const JSON5_CONFIG: &str = "\
// renovate の設定（JSON5）
{
  $schema: 'https://docs.renovatebot.com/renovate-schema.json',
  extends: ['config:recommended'],
  /* 週末だけ */
  schedule: ['every weekend'],
  prHourlyLimit: 0x02,
  timeout: +Infinity,
  ratio: .5,
  \"packageRules\": [
    { matchUpdateTypes: ['minor', 'patch'], automerge: true, },
  ],
}
";
