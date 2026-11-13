; Written for codix: scopes, local definitions and references of C.
; General patterns come first: for a node captured as a reference by
; several patterns, the one written last wins. Macro bodies are text to
; the grammar, so names in them are no references.

; Scopes
;-------

(function_definition) @local.scope.function
(compound_statement) @local.scope.block
(for_statement) @local.scope.block
(field_declaration_list) @local.scope.class
; A prototype's parameters name nothing outside it.
(declaration declarator: (function_declarator) @local.scope.prototype)
(declaration
  declarator: (pointer_declarator declarator: (function_declarator) @local.scope.prototype))

; Definitions
;------------

(parameter_declaration declarator: (identifier) @local.definition.parameter)
(parameter_declaration declarator: (_ declarator: (identifier) @local.definition.parameter))
(parameter_declaration
  declarator: (_ declarator: (_ declarator: (identifier) @local.definition.parameter)))

; A function declarator declares a function, which the `tags` query
; captures; its name here is a reference to it.
(declaration declarator: (identifier) @local.definition.variable)
(declaration declarator: (pointer_declarator declarator: (identifier) @local.definition.variable))
(declaration declarator: (array_declarator declarator: (identifier) @local.definition.variable))
(declaration declarator: (init_declarator declarator: (identifier) @local.definition.variable))
(declaration declarator: (pointer_declarator declarator: (pointer_declarator declarator: (identifier) @local.definition.variable)))
(declaration declarator: (pointer_declarator declarator: (array_declarator declarator: (identifier) @local.definition.variable)))
(declaration declarator: (array_declarator declarator: (pointer_declarator declarator: (identifier) @local.definition.variable)))
(declaration declarator: (array_declarator declarator: (array_declarator declarator: (identifier) @local.definition.variable)))
(declaration declarator: (init_declarator declarator: (pointer_declarator declarator: (identifier) @local.definition.variable)))
(declaration declarator: (init_declarator declarator: (array_declarator declarator: (identifier) @local.definition.variable)))

; References
;-----------

(identifier) @local.reference.read
(type_identifier) @local.reference.type

(call_expression
  function: (identifier) @local.reference.call
  arguments: (argument_list) @local.arguments)
(assignment_expression left: (identifier) @local.reference.write)
(update_expression argument: (identifier) @local.reference.write)

(field_expression
  argument: (_) @local.receiver
  field: (field_identifier) @local.reference.read)
(call_expression
  function: (field_expression
    argument: (_) @local.receiver
    field: (field_identifier) @local.reference.call)
  arguments: (argument_list) @local.arguments)
(assignment_expression
  left: (field_expression
    argument: (_) @local.receiver
    field: (field_identifier) @local.reference.write))

; Names that are no references.
(preproc_params (identifier) @local.skip)
