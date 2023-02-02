; Copyright (c) 2023 Pierric Gimmig. All rights reserved.
; Use of this source code is governed by a BSD-style license that can be
; found in the LICENSE file.

.DATA 
.CODE

; https://msdn.microsoft.com/en-us/library/9z1stfyw.aspx
; Register      Status          Use
; RAX           Volatile        Return value register
; RCX           Volatile        First integer argument
; RDX           Volatile        Second integer argument
; R8            Volatile        Third integer argument
; R9            Volatile        Fourth integer argument
; R10:R11       Volatile        Must be preserved as needed by caller; used in syscall/sysret instructions
; R12:R15       Nonvolatile     Must be preserved by callee
; RDI           Nonvolatile     Must be preserved by callee
; RSI           Nonvolatile     Must be preserved by callee
; RBX           Nonvolatile     Must be preserved by callee
; RBP           Nonvolatile     May be used as a frame pointer; must be preserved by callee
; RSP           Nonvolatile     Stack pointer
; XMM0, YMM0    Volatile        First FP argument; first vector-type argument when __vectorcall is used
; XMM1, YMM1    Volatile        Second FP argument; second vector-type argument when __vectorcall is used
; XMM2, YMM2    Volatile        Third FP argument; third vector-type argument when __vectorcall is used
; XMM3, YMM3    Volatile        Fourth FP argument; fourth vector-type argument when __vectorcall is used
; XMM4, YMM4    Volatile        Must be preserved as needed by caller; fifth vector-type argument when __vectorcall is used
; XMM5, YMM5    Volatile        Must be preserved as needed by caller; sixth vector-type argument when __vectorcall is used
; XMM6:XMM15, YMM6:YMM15        Nonvolatile (XMM), Volatile (upper half of YMM) Must be preserved as needed by callee. YMM registers must be preserved as needed by caller.

HijkPrologAsm PROC
    sub     RSP, 8                  ;// will hold address of trampoline to original function
    push    RCX
    mov     RCX, 0123456789ABCDEFh  ;// will be overwritten with address of prolog data
    jmp     qword ptr[RCX+0]        ;// jump to asm prolog (HijkPrologAsmFixed)
    mov     R11, 0FFFFFFFFFFFFFFFh  ;// Dummy function delimiter, never executed
HijkPrologAsm ENDP
HijkPrologAsmFixed PROC
    push    RBP                     
    mov     RBP, RSP
    push    RDX
    
    mov     RDX, qword ptr[RCX+24]
    mov     qword ptr[RBP+16], RDX  ;// original

    lea     RDX, [RBP+24]           ;// address of return address. Note that RCX contains prolog_data.
    push    RCX                     ;// preserve prolog_data

    sub     RSP, 20h                ;// Shadow space (0x20) - NOTE: stack pointer needs to be aligned on 16 bytes at this point      
    call    qword ptr[RCX+8]        ;// User prolog function  
    add     RSP, 20h

    pop     RCX
    mov     RDX, qword ptr[RCX+16]
    mov     qword ptr[RBP+24], RDX  ;// Overwrite original return address with epilog stub address

    pop     RDX
    pop     RBP
    pop     RCX                     ;// from stub above
    ret                             ;// return to original
HijkPrologAsmFixed  ENDP

HijkEpilogAsm PROC
    sub     RSP, 8                  ;// will hold original caller address
    push    RCX
    mov     RCX, 0123456789ABCDEFh  ;// will be overwritten with address of epilog data
    jmp     qword ptr[RCX+0]        ;// jump to asm epilog (HijkEpilogAsmFixed)
    mov     R11, 0FFFFFFFFFFFFFFFh  ;// Dummy function delimiter, never executed
HijkEpilogAsm ENDP
HijkEpilogAsmFixed PROC  
    push    RBP                     
    mov     RBP, RSP
    push    RDX
    lea     RDX, [RBP+16]           ;// address of original caller address. Note that RCX contains prolog_data.

    sub     RSP, 20h                ;// Shadow space (0x20) - NOTE: stack pointer needs to be aligned on 16 bytes at this point            
    call    qword ptr[RCX+8]        ;// User epilog function  
    add     RSP, 20h

    pop     RDX
    pop     RBP
    pop     RCX
    ret                             ;// Jump to caller through trampoline
HijkEpilogAsmFixed ENDP

HijkGetXmmRegisters PROC
movdqu xmmword ptr[RCX+0*16],  xmm0
movdqu xmmword ptr[RCX+1*16],  xmm1
movdqu xmmword ptr[RCX+2*16],  xmm2
movdqu xmmword ptr[RCX+3*16],  xmm3
movdqu xmmword ptr[RCX+4*16],  xmm4
movdqu xmmword ptr[RCX+5*16],  xmm5
movdqu xmmword ptr[RCX+6*16],  xmm6
movdqu xmmword ptr[RCX+7*16],  xmm7
movdqu xmmword ptr[RCX+8*16],  xmm8
movdqu xmmword ptr[RCX+9*16],  xmm9
movdqu xmmword ptr[RCX+10*16], xmm10
movdqu xmmword ptr[RCX+11*16], xmm11
movdqu xmmword ptr[RCX+12*16], xmm12
movdqu xmmword ptr[RCX+13*16], xmm13
movdqu xmmword ptr[RCX+14*16], xmm14
movdqu xmmword ptr[RCX+15*16], xmm15
ret
HijkGetXmmRegisters ENDP

HijkSetXmmRegisters PROC
movdqu xmm0,  xmmword ptr[RCX+0*16]
movdqu xmm1,  xmmword ptr[RCX+1*16]
movdqu xmm2,  xmmword ptr[RCX+2*16]
movdqu xmm3,  xmmword ptr[RCX+3*16]
movdqu xmm4,  xmmword ptr[RCX+4*16]
movdqu xmm5,  xmmword ptr[RCX+5*16]
movdqu xmm6,  xmmword ptr[RCX+6*16]
movdqu xmm7,  xmmword ptr[RCX+7*16]
movdqu xmm8,  xmmword ptr[RCX+8*16]
movdqu xmm9,  xmmword ptr[RCX+9*16]
movdqu xmm10, xmmword ptr[RCX+10*16]
movdqu xmm11, xmmword ptr[RCX+11*16]
movdqu xmm12, xmmword ptr[RCX+12*16]
movdqu xmm13, xmmword ptr[RCX+13*16]
movdqu xmm14, xmmword ptr[RCX+14*16]
movdqu xmm15, xmmword ptr[RCX+15*16]
ret
HijkSetXmmRegisters ENDP

HijkGetIntegerRegisters PROC
mov qword ptr[RCX+0*8],  RAX
mov qword ptr[RCX+3*8],  RBX
mov qword ptr[RCX+1*8],  RCX
mov qword ptr[RCX+2*8],  RDX
mov qword ptr[RCX+6*8],  RSI
mov qword ptr[RCX+7*8],  RDI
mov qword ptr[RCX+5*8],  RBP
mov qword ptr[RCX+4*8],  RSP
mov qword ptr[RCX+8*8],  R8
mov qword ptr[RCX+9*8],  R9
mov qword ptr[RCX+10*8], R10
mov qword ptr[RCX+11*8], R11
mov qword ptr[RCX+12*8], R12
mov qword ptr[RCX+13*8], R13
mov qword ptr[RCX+14*8], R14
mov qword ptr[RCX+15*8], R15
ret
HijkGetIntegerRegisters ENDP

HijkSetIntegerRegisters PROC
mov RAX, qword ptr[RCX+0*8] 
mov RBX, qword ptr[RCX+1*8] 
mov RCX, qword ptr[RCX+2*8] 
mov RDX, qword ptr[RCX+3*8] 
mov RSI, qword ptr[RCX+4*8] 
mov RDI, qword ptr[RCX+5*8] 
;mov RBP, qword ptr[RCX+6*8] 
;mov RSP, qword ptr[RCX+7*8] 
mov R8,  qword ptr[RCX+8*8] 
mov R9,  qword ptr[RCX+9*8] 
mov R10, qword ptr[RCX+10*8] 
mov R11, qword ptr[RCX+11*8] 
mov R12, qword ptr[RCX+12*8] 
mov R13, qword ptr[RCX+13*8] 
mov R14, qword ptr[RCX+14*8] 
mov R15, qword ptr[RCX+15*8] 
ret
HijkSetIntegerRegisters ENDP

END