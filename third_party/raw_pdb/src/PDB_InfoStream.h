// Copyright 2011-2022, Molecular Matters GmbH <office@molecular-matters.com>
// See LICENSE.txt for licensing details (2-clause BSD License: https://opensource.org/licenses/BSD-2-Clause)

#pragma once

#include "PDB_Types.h"
#include "PDB_CoalescedMSFStream.h"


namespace PDB
{
	class RawFile;


	// PDB Info Stream
	// https://llvm.org/docs/PDB/PdbStream.html
	class PDB_NO_DISCARD InfoStream
	{
	public:
		InfoStream(void) PDB_NO_EXCEPT;
		explicit InfoStream(const RawFile& file) PDB_NO_EXCEPT;

		PDB_DEFAULT_MOVE(InfoStream);

	private:
		CoalescedMSFStream m_stream;
		const Header* m_header;
		bool m_usesDebugFastlink;

		PDB_DISABLE_COPY(InfoStream);
	};
}
