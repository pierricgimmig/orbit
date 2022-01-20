; Copyright (c) 2020 The Orbit Authors. All rights reserved.
; Use of this source code is governed by a BSD-style license that can be
; found in the LICENSE file.

.DATA 
.CODE

; x64 access to thread local storage pointer.
; Could use NtCurrentTeb instead
; https://docs.microsoft.com/en-us/windows/win32/api/winnt/nf-winnt-ntcurrentteb
GetThreadLocalStoragePointer PROC
mov rax, qword ptr gs:[58h]
ret
GetThreadLocalStoragePointer ENDP

END
