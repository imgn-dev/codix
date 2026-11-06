; Added to the TypeScript `locals` query for TSX: lowercase JSX elements
; are HTML tags, not names in scope.

((jsx_opening_element name: (identifier) @local.skip)
  (#match? @local.skip "^[a-z]"))
((jsx_closing_element name: (identifier) @local.skip)
  (#match? @local.skip "^[a-z]"))
((jsx_self_closing_element name: (identifier) @local.skip)
  (#match? @local.skip "^[a-z]"))
