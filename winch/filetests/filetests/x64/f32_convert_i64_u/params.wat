;;! target = "x86_64"

(module
    (func (param i64) (result f32)
        (local.get 0)
        (f32.convert_i64_u)
    )
)
;;      	 55                   	push	rbp
;;      	 4889e5               	mov	rbp, rsp
;;      	 4989fe               	mov	r14, rdi
;;      	 4d8b5e08             	mov	r11, qword ptr [r14 + 8]
;;      	 4d8b1b               	mov	r11, qword ptr [r11]
;;      	 4981c318000000       	add	r11, 0x18
;;      	 4939e3               	cmp	r11, rsp
;;      	 0f874a000000         	ja	0x68
;;   1e:	 4883ec18             	sub	rsp, 0x18
;;      	 48897c2410           	mov	qword ptr [rsp + 0x10], rdi
;;      	 4889742408           	mov	qword ptr [rsp + 8], rsi
;;      	 48891424             	mov	qword ptr [rsp], rdx
;;      	 488b0c24             	mov	rcx, qword ptr [rsp]
;;      	 4883f900             	cmp	rcx, 0
;;      	 0f8c0a000000         	jl	0x48
;;   3e:	 f3480f2ac1           	cvtsi2ss	xmm0, rcx
;;      	 e91a000000           	jmp	0x62
;;   48:	 4989cb               	mov	r11, rcx
;;      	 49c1eb01             	shr	r11, 1
;;      	 4889c8               	mov	rax, rcx
;;      	 4883e001             	and	rax, 1
;;      	 4c09d8               	or	rax, r11
;;      	 f3480f2ac0           	cvtsi2ss	xmm0, rax
;;      	 f30f58c0             	addss	xmm0, xmm0
;;      	 4883c418             	add	rsp, 0x18
;;      	 5d                   	pop	rbp
;;      	 c3                   	ret	
;;   68:	 0f0b                 	ud2	
