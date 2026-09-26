# Layout model data notice

`en-ru-et.aklm` is a trained model derived from word-frequency data. It is
distributed under the Creative Commons Attribution-ShareAlike 4.0
International license (<https://creativecommons.org/licenses/by-sa/4.0/>),
the license of that data. Code that reads it keeps the project's license.

Sources used for training:

- English and Russian word frequencies: wordfreq by Robyn Speer
  (<https://github.com/rspeer/wordfreq>), data licensed CC BY-SA 4.0. wordfreq
  includes data from Google Books Ngrams, the Leeds Internet Corpus,
  Wikipedia, ParaCrawl and other sources listed in its NOTICE.
- Estonian word frequencies: FrequencyWords by Hermit Dave
  (<https://github.com/hermitdave/FrequencyWords>), content licensed
  CC BY-SA 4.0, derived from OpenSubtitles 2018
  (<http://www.opensubtitles.org/>). File `content/2018/et/et_full.txt`,
  SHA-256 `8b0dbb18efd797a497878caae9597fe230be5162dcc6a39d4a35299bb8c60e14`.
- German, French and Spanish words from wordfreq, used only as text that must
  stay unchanged.
- Dictionary membership features use the English, Russian and Estonian word
  lists already documented in `data/language-packs/`.

The model contains no text typed by any user. It can be reproduced with
`tools/layout_model/pipeline.sh`; see `docs/layout-model.md`.
