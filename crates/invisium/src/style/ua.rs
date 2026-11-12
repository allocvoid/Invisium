//! The user-agent stylesheet: default rendering of HTML elements.

pub const UA_CSS: &str = r#"
html, body, div, p, h1, h2, h3, h4, h5, h6, ul, ol, dl, dt, dd, blockquote, pre, form, fieldset,
address, article, aside, footer, header, hgroup, main, nav, section, figure, figcaption, details,
summary, center, hr, menu, dir, legend, listing, xmp, plaintext, search, optgroup { display: block }

head, script, style, title, meta, link, base, template, noframes, noembed, param, source, track,
datalist, area, rp, option, optgroup, [hidden], dialog:not([open]), input[type=hidden] { display: none }

html { color: #000 }
body { margin: 8px }
p, dl { margin: 1em 0 }
blockquote, figure { margin: 1em 40px }
h1 { font-size: 2em; margin: 0.67em 0 }
h2 { font-size: 1.5em; margin: 0.83em 0 }
h3 { font-size: 1.17em; margin: 1em 0 }
h4 { margin: 1.33em 0 }
h5 { font-size: 0.83em; margin: 1.67em 0 }
h6 { font-size: 0.67em; margin: 2.33em 0 }
h1, h2, h3, h4, h5, h6, b, strong, th { font-weight: bold }
i, em, cite, var, dfn, address { font-style: italic }
ul, menu, dir { list-style-type: disc; margin: 1em 0; padding-left: 40px }
ol { list-style-type: decimal; margin: 1em 0; padding-left: 40px }
ul ul, ol ul { list-style-type: circle }
ul ul ul, ol ul ul, ul ol ul, ol ol ul { list-style-type: square }
ul ul, ul ol, ol ul, ol ol { margin-top: 0; margin-bottom: 0 }
li { display: list-item }
dd { margin-left: 40px }
pre, listing, xmp, plaintext { font-family: monospace; white-space: pre; margin: 1em 0 }
code, kbd, samp, tt { font-family: monospace }
u, ins { text-decoration: underline }
s, strike, del { text-decoration: line-through }
small { font-size: smaller }
big { font-size: larger }
sub { vertical-align: sub; font-size: smaller }
sup { vertical-align: super; font-size: smaller }
a:link { color: #0000ee; text-decoration: underline; cursor: pointer }
mark { background-color: #ffff00; color: #000 }
nobr { white-space: nowrap }
center { text-align: -webkit-center }
hr { border: 1px inset #808080; margin: 0.5em auto; color: #808080; overflow: hidden }

table { display: table; border-collapse: separate; border-spacing: 2px; box-sizing: border-box;
  text-indent: 0; border-color: #808080; text-align: left; white-space: normal; line-height: normal;
  font-weight: normal; font-size: medium; font-style: normal }
thead { display: table-header-group; vertical-align: middle }
tbody { display: table-row-group; vertical-align: middle }
tfoot { display: table-footer-group; vertical-align: middle }
tr { display: table-row; vertical-align: inherit }
td, th { display: table-cell; vertical-align: inherit; padding: 1px }
th { text-align: center }
caption { display: table-caption; text-align: center }
colgroup { display: table-column-group }
col { display: table-column }

img, video, canvas, iframe, embed, object, svg, audio { display: inline-block }
svg { overflow: hidden }
iframe { border: 2px inset #808080 }

input, select, textarea, button { display: inline-block; font-size: 13.333px; font-family: sans-serif;
  font-weight: normal; font-style: normal; line-height: normal; color: #000; text-align: left;
  text-transform: none; text-indent: 0; letter-spacing: normal; word-spacing: normal; white-space: nowrap;
  box-sizing: border-box; cursor: text }
input, textarea, select { background-color: #fff; border: 1px solid #767676; padding: 2px 3px; border-radius: 2px }
textarea { font-family: monospace; white-space: pre-wrap; vertical-align: text-bottom; padding: 2px }
select { cursor: default; padding: 1px 20px 1px 4px; border-radius: 3px }
button, input[type=submit], input[type=button], input[type=reset] { background-color: #efefef;
  border: 1px solid #767676; padding: 1px 6px; text-align: center; cursor: default; border-radius: 3px }
input[type=checkbox], input[type=radio] { width: 13px; height: 13px; margin: 3px 3px 3px 4px; padding: 0;
  cursor: default }
input[type=radio] { border-radius: 50% }
input[type=image] { border: 0; padding: 0; background-color: transparent; cursor: pointer }
input[type=range], input[type=color], input[type=file] { border: 0; background-color: transparent }
input:disabled, button:disabled, select:disabled, textarea:disabled { color: #6d6d6d; background-color: #f4f4f4 }
label { cursor: default }
fieldset { border: 2px groove #c0c0c0; padding: 0.35em 0.75em 0.625em; margin: 0 2px }
legend { padding: 0 2px }
details:not([open]) > *:not(summary) { display: none }
summary { cursor: default }
dialog[open] { display: block; position: absolute; left: 0; right: 0; margin: auto; width: fit-content;
  border: 2px solid #000; padding: 1em; background-color: #fff; color: #000 }
ruby rt { font-size: 50% }
"#;

/// Extra rules when scripting is enabled (`<noscript>` content is then inert text).
pub const UA_SCRIPTING_CSS: &str = "noscript { display: none }";
