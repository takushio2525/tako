# shop-app

小さなオンラインショップの API サーバー。tako の紹介動画で使うデモ用プロジェクトです。

## できること

- 商品の一覧と検索（名前の部分一致・ページング）
- カートと注文
- 決済（失敗したら自動でやり直す）

## はじめる

```sh
npm install
npm run dev      # http://localhost:5173
npm test
```

## API

| メソッド | パス | 説明 |
|---|---|---|
| GET | `/api/products` | 商品の一覧 |
| GET | `/api/search?q=` | 商品の検索 |
| POST | `/api/orders` | 注文をつくる |
| POST | `/api/payments` | 決済する |

## 決済のやり直し

通信に失敗したときは、間隔を倍にしながら最大 3 回までやり直します。

```ts
const receipt = await pay(order, { retries: 3 });
```
