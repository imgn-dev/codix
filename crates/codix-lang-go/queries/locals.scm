; Written for codix: scopes, local definitions and references of Go.
; General patterns come first: for a node captured as a reference by
; several patterns, the one written last wins.

; Scopes
;-------

(function_declaration) @local.scope.function
(method_declaration) @local.scope.function
(func_literal) @local.scope.function

(block) @local.scope.block
(if_statement) @local.scope.block
(for_statement) @local.scope.block
(expression_switch_statement) @local.scope.block
(type_switch_statement) @local.scope.block
(select_statement) @local.scope.block
(expression_case) @local.scope.block
(type_case) @local.scope.block
(default_case) @local.scope.block
(communication_case) @local.scope.block

; The key of a composite literal of a named type is a field name for a
; struct type and an expression for a map type; which one needs types.
(composite_literal
  type: [(type_identifier) (qualified_type) (generic_type)]
  body: (literal_value
    (keyed_element
      key: (literal_element) @local.scope.block
      (#set! local.opaque))))

; Definitions
;------------

; A variable is visible after its declaration: in `x := x + 1`, the second
; `x` is the outer one. `:=` reuses a variable already declared in the same
; scope.
; The blank identifier `_` defines nothing.
((short_var_declaration
  left: (expression_list (identifier) @local.definition.variable)) @local.after
  (#not-eq? @local.definition.variable "_")
  (#set! local.reuse))
((var_spec name: (identifier) @local.definition.variable) @local.after
  (#not-eq? @local.definition.variable "_"))
((const_spec name: (identifier) @local.definition.constant) @local.after
  (#not-eq? @local.definition.constant "_"))
(range_clause
  left: (expression_list (identifier) @local.definition.variable)
  ":="
  (#not-eq? @local.definition.variable "_"))
(receive_statement
  left: (expression_list (identifier) @local.definition.variable)
  ":="
  (#not-eq? @local.definition.variable "_"))
(type_switch_statement
  alias: (expression_list (identifier) @local.definition.variable))

((parameter_declaration name: (identifier) @local.definition.parameter)
  (#not-eq? @local.definition.parameter "_"))
((variadic_parameter_declaration name: (identifier) @local.definition.parameter)
  (#not-eq? @local.definition.parameter "_"))
(type_parameter_declaration name: (identifier) @local.definition.type_parameter)

(import_spec name: (package_identifier) @local.definition.import)

; References
;-----------

(identifier) @local.reference.read
(type_identifier) @local.reference.type

(call_expression function: (identifier) @local.reference.call)

(assignment_statement
  left: (expression_list (identifier) @local.reference.write))
(inc_statement (identifier) @local.reference.write)
(dec_statement (identifier) @local.reference.write)
(range_clause
  left: (expression_list (identifier) @local.reference.write)
  "=")

(selector_expression
  operand: (_) @local.receiver
  field: (field_identifier) @local.reference.read)
(call_expression
  function: (selector_expression
    operand: (_) @local.receiver
    field: (field_identifier) @local.reference.call))
(qualified_type
  package: (package_identifier) @local.reference.read)
(qualified_type
  package: (_) @local.receiver
  name: (type_identifier) @local.reference.type)

(composite_literal
  type: (struct_type) @local.receiver
  body: (literal_value
    (keyed_element
      key: (literal_element (identifier) @local.reference.read))))

((identifier) @local.skip
  (#eq? @local.skip "_"))
