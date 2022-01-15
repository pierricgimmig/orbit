; Copyright (c) 2020 The Orbit Authors. All rights reserved.
; Use of this source code is governed by a BSD-style license that can be
; found in the LICENSE file.

.DATA 
.CODE

GetThreadLocalStoragePointer PROC
mov rax, qword ptr gs:[58h]
ret
GetThreadLocalStoragePointer ENDP

END
