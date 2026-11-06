; Written for codix: scopes, local definitions and references of
; JavaScript. General patterns come first: for a node captured as a
; reference by several patterns, the one written last wins.

; Scopes
;-------

; `let`, `const`, classes and functions are visible in their whole block,
; so every scope is hoisted. `var` belongs to its function.
(function_declaration) @local.scope.function
(generator_function_declaration) @local.scope.function
(function_expression) @local.scope.function
(generator_function) @local.scope.function
(arrow_function) @local.scope.function
(method_definition) @local.scope.function
(class_body) @local.scope.class
(statement_block) @local.scope.block
(for_statement) @local.scope.block
(for_in_statement) @local.scope.block
(catch_clause) @local.scope.block

; Inside `with`, any name may be a property of its object.
(with_statement
  body: (_) @local.scope.block
  (#set! local.opaque))

; Definitions
;------------

((variable_declaration
  (variable_declarator name: (identifier) @local.definition.variable))
  (#set! local.scope "function"))
(lexical_declaration
  (variable_declarator name: (identifier) @local.definition.variable))
(for_in_statement
  kind: _
  left: (identifier) @local.definition.variable)

(formal_parameters (identifier) @local.definition.parameter)
(formal_parameters (assignment_pattern left: (identifier) @local.definition.parameter))
(formal_parameters (rest_pattern (identifier) @local.definition.parameter))
(arrow_function parameter: (identifier) @local.definition.parameter)
(catch_clause parameter: (identifier) @local.definition.parameter)

(object_pattern (shorthand_property_identifier_pattern) @local.definition.variable)
(object_pattern (pair_pattern value: (identifier) @local.definition.variable))
(object_pattern (object_assignment_pattern left: (shorthand_property_identifier_pattern) @local.definition.variable))
(array_pattern (identifier) @local.definition.variable)
(array_pattern (assignment_pattern left: (identifier) @local.definition.variable))

(import_clause (identifier) @local.definition.import)
(import_specifier !alias name: (identifier) @local.definition.import)
(import_specifier alias: (identifier) @local.definition.import)
(namespace_import (identifier) @local.definition.import)

; References
;-----------

(identifier) @local.reference.read
(shorthand_property_identifier) @local.reference.read

(call_expression function: (identifier) @local.reference.call)
(assignment_expression left: (identifier) @local.reference.write)
(augmented_assignment_expression left: (identifier) @local.reference.write)
(update_expression argument: (identifier) @local.reference.write)

(member_expression
  object: (_) @local.receiver
  property: (property_identifier) @local.reference.read)
(call_expression
  function: (member_expression
    object: (_) @local.receiver
    property: (property_identifier) @local.reference.call))
(assignment_expression
  left: (member_expression
    object: (_) @local.receiver
    property: (property_identifier) @local.reference.write))

; Names that are no references: the name a module exports under an alias,
; and lowercase JSX elements, which are HTML tags.
(import_specifier name: (identifier) @local.skip alias: (_))
(export_specifier alias: (identifier) @local.skip)
((jsx_opening_element name: (identifier) @local.skip)
  (#match? @local.skip "^[a-z]"))
((jsx_closing_element name: (identifier) @local.skip)
  (#match? @local.skip "^[a-z]"))
((jsx_self_closing_element name: (identifier) @local.skip)
  (#match? @local.skip "^[a-z]"))
