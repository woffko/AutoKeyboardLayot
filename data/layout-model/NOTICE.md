# Layout model data notice

`en-ru-et.aklm` is a trained model derived from word-frequency data. It is
distributed under the Creative Commons Attribution-ShareAlike 4.0
International license (<https://creativecommons.org/licenses/by-sa/4.0/>), the
license of that data. Code that reads it keeps the project's license.

Sources used for training:

- English and Russian word frequencies: wordfreq 3.1.1 by Robyn Speer
  (<https://github.com/rspeer/wordfreq>), data licensed CC BY-SA 4.0. wordfreq
  includes data from Google Books Ngrams, the Leeds Internet Corpus,
  Wikipedia, ParaCrawl and other sources listed in its NOTICE.
- Estonian word frequencies: FrequencyWords by Hermit Dave
  (<https://github.com/hermitdave/FrequencyWords>), content licensed CC BY-SA
  4.0, derived from OpenSubtitles 2018 (<http://www.opensubtitles.org/>). File
  `content/2018/et/et_full.txt`, SHA-256
  `8b0dbb18efd797a497878caae9597fe230be5162dcc6a39d4a35299bb8c60e14`.
- German, French and Spanish words from wordfreq 3.1.1, used only as text that
  must stay unchanged.
- Dictionary membership features use the English, Russian and Estonian word
  lists already documented in `data/language-packs/`.

The wordfreq data used here includes SUBTLEX word lists (SUBTLEX-US,
SUBTLEX-UK for English and SUBTLEX-DE for German) created by Marc Brysbaert et
al. SUBTLEX is freely available data
(<http://crr.ugent.be/programs-data/subtitle-frequencies>), which wordfreq
distributes with his permission on condition that its authors are credited:

- Brysbaert, M. & New, B. (2009). Moving beyond Kucera and Francis: A Critical
  Evaluation of Current Word Frequency Norms and the Introduction of a New and
  Improved Word Frequency Measure for American English. Behavior Research
  Methods, 41 (4), 977-990.
- van Heuven, W. J., Mandera, P., Keuleers, E., & Brysbaert, M. (2014).
  SUBTLEX-UK: A new and improved word frequency database for British English.
  The Quarterly Journal of Experimental Psychology, 67(6), 1176-1190.
- Brysbaert, M., Buchmeier, M., Conrad, M., Jacobs, A.M., Bölte, J., & Böhl,
  A. (2011). The word frequency effect: A review of recent developments and
  implications for the choice of frequency estimates in German. Experimental
  Psychology, 58, 412-424.

The model contains no text typed by any user. It is built with
`tools/layout_model/pipeline.sh`; see `docs/layout-model.md` for the steps and
for what a rebuild reproduces.
