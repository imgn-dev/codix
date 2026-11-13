; Written for codix: scopes, local definitions and references of C++,
; after the C pack's.
; General patterns come first: for a node captured as a reference by
; several patterns, the one written last wins. Macro bodies are text to
; the grammar, so names in them are no references.

; Scopes
;-------

(function_definition) @local.scope.function
(compound_statement) @local.scope.block
(for_statement) @local.scope.block
; Class members are visible in the whole class. A class with bases may
; inherit members codix cannot see, and so may a member function defined
; out of its class, whose class's members are in scope in its body.
(field_declaration_list) @local.scope.class
(class_specifier
  (base_class_clause)
  body: (field_declaration_list) @local.scope.class
  (#set! local.opaque))
(struct_specifier
  (base_class_clause)
  body: (field_declaration_list) @local.scope.class
  (#set! local.opaque))
(function_definition
  declarator: (function_declarator declarator: (qualified_identifier))
  body: (compound_statement) @local.scope.block
  (#set! local.opaque))
(namespace_definition body: (declaration_list) @local.scope.module)
(lambda_expression) @local.scope.function
(for_range_loop) @local.scope.block
(catch_clause) @local.scope.block
; Template parameters are visible in the whole template, whose class or
; function belongs to the scope around it.
((template_declaration) @local.scope.type
  (#set! local.transparent))
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
(parameter_declaration declarator: (reference_declarator (identifier) @local.definition.parameter))
(optional_parameter_declaration declarator: (identifier) @local.definition.parameter)
(optional_parameter_declaration
  declarator: (reference_declarator (identifier) @local.definition.parameter))
(for_range_loop declarator: (identifier) @local.definition.variable)
(for_range_loop declarator: (reference_declarator (identifier) @local.definition.variable))
(type_parameter_declaration (type_identifier) @local.definition.type_parameter)

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

(qualified_identifier
  scope: (_) @local.receiver
  name: (identifier) @local.reference.read)
(call_expression
  function: (qualified_identifier
    scope: (_) @local.receiver
    name: (identifier) @local.reference.call))

; Names that are no references.
(preproc_params (identifier) @local.skip)
(destructor_name (identifier) @local.skip)
