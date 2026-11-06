; Written for codix: scopes, local definitions and references of Ruby.
; General patterns come first: for a node captured as a reference by
; several patterns, the one written last wins.

; Scopes
;-------

; A method, class or module body does not see the local variables around
; it; a block does. Instance variables belong to their class.
(method) @local.scope.function
(singleton_method) @local.scope.function
(class body: (body_statement) @local.scope.class)
(module body: (body_statement) @local.scope.class)
(singleton_class body: (body_statement) @local.scope.class)
(block) @local.scope.block
(do_block) @local.scope.block
(lambda) @local.scope.block

; Definitions
;------------

; A name is a local variable from its first assignment on; before it, and
; where nothing assigns it, a bare name calls a method.
((assignment left: (identifier) @local.definition.variable)
  (#set! local.reuse "visible"))
((assignment left: (left_assignment_list (identifier) @local.definition.variable))
  (#set! local.reuse "visible"))
((operator_assignment left: (identifier) @local.definition.variable)
  (#set! local.reuse "visible"))
((for pattern: (identifier) @local.definition.variable)
  (#set! local.reuse "visible"))
(exception_variable (identifier) @local.definition.variable)

((assignment left: (instance_variable) @local.definition.ivar)
  (#set! local.scope "class")
  (#set! local.reuse))
((operator_assignment left: (instance_variable) @local.definition.ivar)
  (#set! local.scope "class")
  (#set! local.reuse))

(method_parameters (identifier) @local.definition.parameter)
(block_parameters (identifier) @local.definition.parameter)
(lambda_parameters (identifier) @local.definition.parameter)
(optional_parameter name: (identifier) @local.definition.parameter)
(keyword_parameter name: (identifier) @local.definition.parameter)
(splat_parameter name: (identifier) @local.definition.parameter)
(hash_splat_parameter name: (identifier) @local.definition.parameter)
(block_parameter name: (identifier) @local.definition.parameter)

; References
;-----------

(identifier) @local.reference.read
(constant) @local.reference.read
(instance_variable) @local.reference.read

(call
  !receiver
  method: (identifier) @local.reference.call)
(call
  receiver: (_) @local.receiver
  method: (identifier) @local.reference.call)
(call
  receiver: (_) @local.receiver
  method: (constant) @local.reference.call)
(scope_resolution
  scope: (_) @local.receiver
  name: (constant) @local.reference.read)
