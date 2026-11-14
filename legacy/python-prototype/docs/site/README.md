# Сайт документації Oberih

Статичний односторінковий сайт (без залежностей, без збірки).

## Перегляд локально

```bash
cd docs/site
python3 -m http.server 8000
```

Відкрий `http://localhost:8000` в браузері.

## Хостинг на GitHub Pages

1. У налаштуваннях репозиторію: Settings → Pages
2. Source: "Deploy from a branch", branch: `main`, папка: `/docs/site`
   (або перенеси вміст `docs/site/` у корінь окремої `gh-pages` гілки)
3. Сайт буде доступний на `https://<username>.github.io/<repo>/`
