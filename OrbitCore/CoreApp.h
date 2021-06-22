//-----------------------------------
// Copyright Pierric Gimmig 2013-2017
//-----------------------------------
#pragma once

#include <string>
#include <vector>
#include <memory>
#include <unordered_map>
#include "BaseTypes.h"

class CoreApp
{
public:
    virtual void SendToUiAsync( const std::wstring & /*a_Msg*/ ){}
    virtual void SendToUiNow( const std::wstring & /*a_Msg*/ ){}
    virtual bool GetUnrealSupportEnabled(){ return false; }
    virtual bool GetUnitySupportEnabled(){ return false; }
    virtual bool GetUnsafeHookingEnabled(){ return true; }
    virtual bool GetSamplingEnabled(){ return false; }
    virtual bool GetOutputDebugStringEnabled(){ return false; }
    virtual void LogMsg( const std::wstring & /*a_Msg*/ ){}
    virtual void UpdateVariable( class Variable * /*a_Variable*/ ){}
    virtual void Disassemble( const std::string & /*a_FunctionName*/, DWORD64 /*a_VirtualAddress*/, const char * /*a_MachineCode*/, int /*a_Size*/ ){}
    virtual const std::unordered_map<DWORD64, std::shared_ptr<class Rule> >* GetRules(){ return nullptr; }

    std::vector< std::wstring > m_SymbolLocations;
};

extern CoreApp* GCoreApp;
