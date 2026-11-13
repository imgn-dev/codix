; Adapted from tree-sitter-javascript 0.25.0 and tree-sitter-typescript
; 0.23.2, queries/tags.scm: TypeScript's query extends JavaScript's, so both
; are here. Doc comment captures are dropped; constructors get their own kind.

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

(function_signature
  name: (identifier) @name) @definition.function

(method_signature
  name: (property_identifier) @name) @definition.method

(abstract_method_signature
  name: (property_identifier) @name) @definition.method

(abstract_class_declaration
  name: (type_identifier) @name) @definition.class

(module
  name: (identifier) @name) @definition.module

(interface_declaration
  name: (type_identifier) @name) @definition.interface

(type_annotation
  (type_identifier) @name) @reference.type

; Added for codix.

(method_definition
  name: (property_identifier) @name
  (#eq? @name "constructor")) @definition.constructor

(internal_module
  name: (identifier) @name) @definition.namespace

(type_alias_declaration
  name: (type_identifier) @name) @definition.type_alias

(enum_declaration
  name: (identifier) @name) @definition.enum

(enum_body
  name: (property_identifier) @name @definition.enum_member)

(enum_assignment
  name: (property_identifier) @name) @definition.enum_member

(public_field_definition
  name: (property_identifier) @name) @definition.field

(property_signature
  name: (property_identifier) @name) @definition.property

; Top-level constants and variables whose value is not a function.
(program
  [
    (lexical_declaration
      kind: "const"
      (variable_declarator
        name: (identifier) @name
        !value) @definition.constant)
    (lexical_declaration
      kind: "const"
      (variable_declarator
        name: (identifier) @name
        value: [(number) (string) (template_string) (object) (array) (true) (false) (null) (identifier) (member_expression) (call_expression) (new_expression) (binary_expression) (as_expression)]) @definition.constant)
    (export_statement
      (lexical_declaration
        kind: "const"
        (variable_declarator
          name: (identifier) @name
          value: [(number) (string) (template_string) (object) (array) (true) (false) (null) (identifier) (member_expression) (call_expression) (new_expression) (binary_expression) (as_expression)]) @definition.constant))
  ])

; Added for codix: supertypes, as written.

(class_declaration
  (class_heritage (extends_clause value: (_) @supertype))) @supertypes

(abstract_class_declaration
  (class_heritage (extends_clause value: (_) @supertype))) @supertypes

(class
  (class_heritage (extends_clause value: (_) @supertype))) @supertypes

(class_declaration
  (class_heritage (implements_clause (_) @supertype))) @supertypes

(abstract_class_declaration
  (class_heritage (implements_clause (_) @supertype))) @supertypes

(class
  (class_heritage (implements_clause (_) @supertype))) @supertypes

(interface_declaration
  (extends_type_clause (_) @supertype)) @supertypes
