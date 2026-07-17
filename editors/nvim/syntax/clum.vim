if exists('b:current_syntax')
  finish
endif

syntax sync fromstart

syntax match clumComment "//.*$"

syntax match clumTypeName /\v<[A-Z][A-Za-z0-9]*>/

syntax match clumDeclMark /\v^\s*\zs#\ze\s/
syntax match clumImportMark /\v^\s*\zs\@/
syntax match clumImportPath /\v%(^\s*\@)@<=\S+/
syntax match clumExportMark /\v^\zs\^\ze[A-Za-z]/
syntax match clumExportName /\v%(^\^)@<=[a-z][a-z0-9-]*/
syntax match clumListMarker /\v^\s*\zs-\ze\s/

syntax match clumOperator "|>"
syntax match clumOperator "->"
syntax match clumOperator /\v\s@<=\=%(\s|$)@=/
syntax match clumBang /\v%(^|\s)@<=!%(\s|$)@=/
syntax match clumColonWord /\v%(^|\s)@<=:[a-z][a-z0-9-]*/
syntax match clumFieldName /\v<[a-z][a-z0-9-]*\ze:%(\s|$)/

syntax match clumH /\v%(^|\s)@<=h\ze\s+\./
syntax match clumTagLiteral /\v%(^|\s)@<=\.[a-z][a-z0-9-]*>/

syntax match clumNumber /\v%(^|[ \t,([{])@<=-?\d+%(\.\d+)?>/

syntax region clumEmbed matchgroup=clumEmbedBrace start=/{/ end=/}/ oneline contains=clumString,clumNumber,clumTypeName,clumColonWord
syntax match clumBraceEscape /{{\|}}/

syntax region clumStringInterp matchgroup=clumEmbedBrace start=/{/ end=/}/ contained oneline contains=clumNumber,clumTypeName
syntax match clumStringEscape /{{\|}}/ contained
syntax region clumString start=/'/ end=/'/ oneline contains=clumStringInterp,clumStringEscape

highlight default link clumComment Comment
highlight default link clumString String
highlight default link clumStringEscape SpecialChar
highlight default link clumBraceEscape SpecialChar
highlight default link clumEmbedBrace Special
highlight default link clumNumber Number
highlight default link clumTypeName Type
highlight default link clumH Statement
highlight default link clumTagLiteral Constant
highlight default link clumDeclMark PreProc
highlight default link clumImportMark Include
highlight default link clumImportPath String
highlight default link clumExportMark Include
highlight default link clumExportName Identifier
highlight default link clumListMarker Special
highlight default link clumOperator Operator
highlight default link clumBang Operator
highlight default link clumColonWord Keyword
highlight default link clumFieldName Identifier

let b:current_syntax = 'clum'
