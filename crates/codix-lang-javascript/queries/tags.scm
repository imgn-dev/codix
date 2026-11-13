; Adapted from tree-sitter-javascript 0.25.0, queries/tags.scm: doc comment
; captures are dropped; constructors get their own kind.

(method_definition
  name: (property_identifier) @name
  (#not-eq? @name "constructor")) @definition.method

[
  (class
    name: (_) @name)
  (class_declaration
    name: (_) @name)
] @definition.class

[
  (function_expression
    name: (identifier) @name)
  (function_declaration
    name: (identifier) @name)
  (generator_function
    name: (identifier) @name)
  (generator_function_declaration
    name: (identifier) @name)
] @definition.function

(lexical_declaration
  (variable_declarator
    name: (identifier) @name
    value: [(arrow_function) (function_expression)]) @definition.function)

(variable_declaration
  (variable_declarator
    name: (identifier) @name
    value: [(arrow_function) (function_expression)]) @definition.function)

(assignment_expression
  left: [
    (identifier) @name
    (member_expression
      property: (property_identifier) @name)
  ]
  right: [(arrow_function) (function_expression)]) @definition.function

(pair
  key: (property_identifier) @name
  value: [(arrow_function) (function_expression)]) @definition.function

((call_expression
  function: (identifier) @name) @reference.call
  (#not-match? @name "^(require)$"))

(call_expression
  function: (member_expression
    property: (property_identifier) @name)
  arguments: (_) @reference.call)

(new_expression
  constructor: (_) @name) @reference.class

; Added for codix.

(method_definition
  name: (property_identifier) @name
  (#eq? @name "constructor")) @definition.constructor

(field_definition
  property: [(property_identifier) (private_property_identifier)] @name) @definition.field

; Top-level constants whose value is not a function.
(program
  [
    (lexical_declaration
      kind: "const"
      (variable_declarator
        name: (identifier) @name
        value: [(number) (string) (template_string) (object) (array) (true) (false) (null) (identifier) (member_expression) (call_expression) (new_expression) (binary_expression)]) @definition.constant)
    (export_statement
      (lexical_declaration
        kind: "const"
        (variable_declarator
          name: (identifier) @name
          value: [(number) (string) (template_string) (object) (array) (true) (false) (null) (identifier) (member_expression) (call_expression) (new_expression) (binary_expression)]) @definition.constant))
  ])

; Added for codix: supertypes, as written.

(class_declaration
  (class_heritage (_) @supertype)) @supertypes

(class
  (class_heritage (_) @supertype)) @supertypes
