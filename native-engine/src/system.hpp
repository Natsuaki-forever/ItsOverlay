#pragma once
#include "core.hpp"
#include "application_profiles.hpp"
#include <shlobj.h>

inline fs::path application_file(){
    wchar_t text[32768];DWORD size=GetModuleFileNameW(nullptr,text,32768);
    if(!size||size>=32768)throw std::runtime_error("Cannot locate application directory");
    return fs::path(std::wstring(text,size));
}
inline fs::path application_data(){
    wchar_t text[MAX_PATH];
    check(SHGetFolderPathW(nullptr,CSIDL_APPDATA,nullptr,SHGFP_TYPE_CURRENT,text));
    return fs::path(text)/L"ChatGPTOverlay";
}
inline void record_error(const std::string& text) noexcept{
    try{
        auto directory=application_data();fs::create_directories(directory);
        std::ofstream(directory/L"native-error.log",std::ios::app)<<text<<'\n';
    }catch(...){}
}

struct NativeShell {
    static constexpr UINT callback=WM_APP+60;
    static constexpr int hotkeyId=0x4347;
    enum Command:UINT {Edit=100,Rediscover,OpenConfig,Startup,Help,Quit,PickApplication,TargetFirst=1000,ApplicationFirst=2000};
    HWND host=nullptr;
    HICON icon=nullptr;
    bool added=false,hotkey=false,test=false;
    std::string hotkeySpec="Ctrl+Alt+O";
    UINT taskbarCreated=0;
    std::vector<HWND> targets;
    std::vector<WindowIdentity> applicationWindows;
    std::vector<std::string> applicationIds;
    static constexpr const wchar_t* runKey=L"Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    static constexpr const wchar_t* prefsKey=L"Software\\ChatGPTOverlay";
    static std::string load_saved_hotkey(){
        HKEY key=nullptr;
        if(RegOpenKeyExW(HKEY_CURRENT_USER,prefsKey,0,KEY_QUERY_VALUE,&key)!=ERROR_SUCCESS)return "Ctrl+Alt+O";
        wchar_t buf[128]{};DWORD size=sizeof(buf);DWORD type=0;
        LONG res=RegQueryValueExW(key,L"Hotkey",nullptr,&type,reinterpret_cast<BYTE*>(buf),&size);
        RegCloseKey(key);
        if(res!=ERROR_SUCCESS||type!=REG_SZ||size<sizeof(wchar_t))return "Ctrl+Alt+O";
        std::string loaded=utf8(std::wstring(buf));
        return loaded.empty()?"None":loaded;
    }
    static void save_hotkey(const std::string& spec){
        HKEY key=nullptr;
        if(RegCreateKeyExW(HKEY_CURRENT_USER,prefsKey,0,nullptr,0,KEY_SET_VALUE,nullptr,&key,nullptr)!=ERROR_SUCCESS)return;
        auto w=wide(spec);
        RegSetValueExW(key,L"Hotkey",0,REG_SZ,reinterpret_cast<const BYTE*>(w.c_str()),DWORD((w.size()+1)*sizeof(wchar_t)));
        RegCloseKey(key);
    }
    static bool parse_hotkey(const std::string& spec,UINT& mods,UINT& vk,std::string& normalized){
        mods=0;vk=0;normalized.clear();
        std::string trimmed;for(char c:spec)if(!std::isspace(static_cast<unsigned char>(c)))trimmed.push_back(c);
        if(trimmed.empty()||_stricmp(trimmed.c_str(),"none")==0){normalized="None";return true;}
        std::vector<std::string> parts;size_t pos=0;
        while(pos<=trimmed.size()){
            size_t next=trimmed.find('+',pos);
            if(next==std::string::npos){parts.push_back(trimmed.substr(pos));break;}
            parts.push_back(trimmed.substr(pos,next-pos));pos=next+1;
        }
        if(parts.empty())return false;
        bool hasCtrl=false,hasAlt=false,hasShift=false;
        for(size_t i=0;i+1<parts.size();i++){
            const auto& p=parts[i];
            if(_stricmp(p.c_str(),"ctrl")==0||_stricmp(p.c_str(),"control")==0)hasCtrl=true;
            else if(_stricmp(p.c_str(),"alt")==0)hasAlt=true;
            else if(_stricmp(p.c_str(),"shift")==0)hasShift=true;
            else return false;
        }
        const auto& k=parts.back();
        if(k.size()==1){
            char ch=char(std::toupper(static_cast<unsigned char>(k[0])));
            if((ch>='A'&&ch<='Z')||(ch>='0'&&ch<='9'))vk=UINT(ch);
            else return false;
        }else if((k[0]=='F'||k[0]=='f')&&k.size()<=3){
            int fn=0;try{fn=std::stoi(k.substr(1));}catch(...){return false;}
            if(fn>=1&&fn<=12)vk=UINT(VK_F1+fn-1);else return false;
        }else return false;
        if(!hasCtrl&&!hasAlt&&!hasShift&&!(vk>=VK_F1&&vk<=VK_F12))return false;
        if(hasCtrl){mods|=MOD_CONTROL;normalized+="Ctrl+";}
        if(hasAlt){mods|=MOD_ALT;normalized+="Alt+";}
        if(hasShift){mods|=MOD_SHIFT;normalized+="Shift+";}
        if(vk>=VK_F1&&vk<=VK_F12)normalized+="F"+std::to_string(int(vk-VK_F1+1));
        else normalized.push_back(char(vk));
        return true;
    }
    std::wstring tray_tip()const{
        if(hotkeySpec=="None"||hotkeySpec.empty())return L"ItsOverlay - 就是遮";
        return L"ItsOverlay - 就是遮 · "+wide(hotkeySpec);
    }
    NOTIFYICONDATAW data()const{
        NOTIFYICONDATAW value{};value.cbSize=sizeof(value);value.hWnd=host;value.uID=1;return value;
    }
    static HICON make_icon(){
        HICON loaded=static_cast<HICON>(LoadImageW(GetModuleHandleW(nullptr),MAKEINTRESOURCEW(1),IMAGE_ICON,
            GetSystemMetrics(SM_CXSMICON),GetSystemMetrics(SM_CYSMICON),LR_DEFAULTCOLOR));
        if(loaded)return loaded;
        BITMAPV5HEADER header{};header.bV5Size=sizeof(header);header.bV5Width=32;header.bV5Height=-32;
        header.bV5Planes=1;header.bV5BitCount=32;header.bV5Compression=BI_BITFIELDS;
        header.bV5RedMask=0x00ff0000;header.bV5GreenMask=0x0000ff00;header.bV5BlueMask=0xff;header.bV5AlphaMask=0xff000000;
        void* pixels=nullptr;HBITMAP bitmap=CreateDIBSection(nullptr,reinterpret_cast<BITMAPINFO*>(&header),DIB_RGB_COLORS,&pixels,nullptr,0);
        if(!bitmap)return nullptr;
        for(int y=0;y<32;y++)for(int x=0;x<32;x++){
            double distance=std::hypot(x-15.5,y-15.5);
            static_cast<DWORD*>(pixels)[y*32+x]=(distance>=8&&distance<=13)?0xff8bdcc0:0;
        }
        HBITMAP mask=CreateBitmap(32,32,1,1,nullptr);
        ICONINFO info{};info.fIcon=TRUE;info.hbmColor=bitmap;info.hbmMask=mask;
        HICON result=mask?CreateIconIndirect(&info):nullptr;
        if(mask)DeleteObject(mask);DeleteObject(bitmap);return result;
    }
    bool add(){
        auto value=data();value.uFlags=NIF_MESSAGE|NIF_ICON|NIF_TIP|NIF_SHOWTIP;
        value.uCallbackMessage=callback;value.hIcon=icon;
        wcsncpy_s(value.szTip,tray_tip().c_str(),_TRUNCATE);
        added=Shell_NotifyIconW(NIM_ADD,&value)!=FALSE;
        if(added){value.uVersion=NOTIFYICON_VERSION_4;Shell_NotifyIconW(NIM_SETVERSION,&value);}
        return added;
    }
    bool apply_hotkey(const std::string& spec,bool persist=true){
        if(hotkey){UnregisterHotKey(host,hotkeyId);hotkey=false;}
        if(test){
            hotkey=RegisterHotKey(host,hotkeyId,MOD_CONTROL|MOD_SHIFT|MOD_NOREPEAT,VK_F24)!=FALSE;
            return hotkey;
        }
        UINT mods=0,vk=0;std::string normalized;
        if(!parse_hotkey(spec,mods,vk,normalized)){
            if(!persist){hotkeySpec="None";return false;}
            UINT oldM=0,oldV=0;std::string oldN;
            if(parse_hotkey(hotkeySpec,oldM,oldV,oldN)&&oldN!="None")
                hotkey=RegisterHotKey(host,hotkeyId,oldM|MOD_NOREPEAT,oldV)!=FALSE;
            throw std::runtime_error("invalid_hotkey");
        }
        if(normalized=="None"){
            hotkeySpec="None";
            if(persist)save_hotkey(hotkeySpec);
            calibration_status(L"");
            return true;
        }
        if(!RegisterHotKey(host,hotkeyId,mods|MOD_NOREPEAT,vk)){
            if(persist){
                UINT oldM=0,oldV=0;std::string oldN;
                if(parse_hotkey(hotkeySpec,oldM,oldV,oldN)&&oldN!="None")
                    hotkey=RegisterHotKey(host,hotkeyId,oldM|MOD_NOREPEAT,oldV)!=FALSE;
                throw std::runtime_error("hotkey_in_use");
            }
            hotkeySpec=normalized;
            return false;
        }
        hotkey=true;hotkeySpec=normalized;
        if(persist)save_hotkey(hotkeySpec);
        calibration_status(L"");
        return true;
    }
    void start(HWND window,bool testing){
        host=window;test=testing;icon=make_icon();
        if(!icon)throw std::runtime_error("Cannot create tray icon");
        taskbarCreated=RegisterWindowMessageW(L"TaskbarCreated");
        std::string saved=test?std::string("Ctrl+Shift+F24"):load_saved_hotkey();
        hotkeySpec=saved;
        add();
        apply_hotkey(saved,false);
        if(!added)SetTimer(host,9,2000,nullptr);
        if(!hotkey&&hotkeySpec!="None")notice(L"快捷键 "+wide(hotkeySpec)+L" 已被其他程序占用，可在设置中修改或从托盘进入编辑。",true);
    }
    void calibration_status(const std::wstring& text){
        if(!added)return;
        auto value=data();value.uFlags=NIF_TIP|NIF_SHOWTIP;
        wcsncpy_s(value.szTip,text.empty()?tray_tip().c_str():text.c_str(),_TRUNCATE);
        Shell_NotifyIconW(NIM_MODIFY,&value);
    }
    void notice(const std::wstring& text,bool error=false){
        if(!added){record_error(utf8(text));return;}
        auto value=data();value.uFlags=NIF_INFO;value.dwInfoFlags=error?NIIF_WARNING:NIIF_INFO;
        wcsncpy_s(value.szInfoTitle,L"ItsOverlay - 就是遮",_TRUNCATE);
        wcsncpy_s(value.szInfo,text.c_str(),_TRUNCATE);Shell_NotifyIconW(NIM_MODIFY,&value);
    }
    bool startup_enabled()const{
        HKEY key=nullptr;if(RegOpenKeyExW(HKEY_CURRENT_USER,runKey,0,KEY_QUERY_VALUE,&key)!=ERROR_SUCCESS)return false;
        LONG result=RegQueryValueExW(key,L"ChatGPTOverlay",nullptr,nullptr,nullptr,nullptr);RegCloseKey(key);return result==ERROR_SUCCESS;
    }
    void toggle_startup(){
        if(test)throw std::runtime_error("Startup changes are disabled in diagnostic mode");
        bool enabled=startup_enabled();HKEY key=nullptr;
        LONG result=RegCreateKeyExW(HKEY_CURRENT_USER,runKey,0,nullptr,0,KEY_SET_VALUE,nullptr,&key,nullptr);
        if(result!=ERROR_SUCCESS)throw std::runtime_error("Cannot open startup settings");
        std::wstring command=L"\""+application_file().wstring()+L"\"";
        result=enabled?RegDeleteValueW(key,L"ChatGPTOverlay"):
            RegSetValueExW(key,L"ChatGPTOverlay",0,REG_SZ,reinterpret_cast<const BYTE*>(command.c_str()),DWORD((command.size()+1)*sizeof(wchar_t)));
        RegCloseKey(key);if(result!=ERROR_SUCCESS)throw std::runtime_error("Cannot update startup settings");
    }
    UINT menu(HWND selected,bool editing,const ApplicationProfiles* profiles=nullptr){
        HMENU menu=CreatePopupMenu();if(!menu)throw std::runtime_error("Cannot create tray menu");
        targets.clear();
        if(!profiles)EnumWindows([](HWND window,LPARAM param)->BOOL{
            if(available(window)&&!GetWindow(window,GW_OWNER)&&is_chatgpt(window))
                reinterpret_cast<NativeShell*>(param)->targets.push_back(window);
            return TRUE;
        },reinterpret_cast<LPARAM>(this));
        std::wstring editLabel=(hotkeySpec=="None"||hotkeySpec.empty())?L"打开／恢复编辑器":(L"打开／恢复编辑器\t"+wide(hotkeySpec));
        AppendMenuW(menu,MF_STRING,Edit,editLabel.c_str());
        if(profiles){
            AppendMenuW(menu,MF_STRING,PickApplication,L"点选并保存应用窗口…");
            HMENU saved=CreatePopupMenu();applicationIds.clear();
            auto active=profiles->active();
            for(const auto& app:profiles->document["applications"]){
                if(!ApplicationProfiles::listed(app))continue;
                auto id=app.at("id").get<std::string>();
                auto label=wide(app.at("name").get<std::string>());
                if(id!="chatgpt")label+=L" · "+wide(app.at("executable").get<std::string>());
                AppendMenuW(saved,MF_STRING|(id==active?MF_CHECKED:0),ApplicationFirst+applicationIds.size(),label.c_str());
                applicationIds.push_back(std::move(id));
            }
            AppendMenuW(menu,MF_POPUP,reinterpret_cast<UINT_PTR>(saved),L"已保存的应用");
            applicationWindows=profiles->candidates(active);
            if(!applicationWindows.empty()){
                HMENU choices=CreatePopupMenu();
                if(applicationWindows.size()>ApplicationFirst-TargetFirst)applicationWindows.resize(ApplicationFirst-TargetFirst);
                for(size_t i=0;i<applicationWindows.size();i++){
                    auto& candidate=applicationWindows[i];wchar_t title[256]{};GetWindowTextW(candidate.window,title,256);
                    std::wstring label=std::to_wstring(i+1)+L". "+(title[0]?title:L"无标题")+L" (PID "+std::to_wstring(candidate.pid)+L")";
                    AppendMenuW(choices,MF_STRING|(candidate.window==selected?MF_CHECKED:0),TargetFirst+i,label.c_str());
                }
                AppendMenuW(menu,MF_POPUP,reinterpret_cast<UINT_PTR>(choices),L"绑定当前应用的窗口");
            }
        }else AppendMenuW(menu,MF_STRING,Rediscover,L"重新识别 ChatGPT");
        if(!profiles&&targets.size()>1){
            HMENU choices=CreatePopupMenu();
            for(size_t i=0;i<targets.size();i++){
                wchar_t title[256]{};GetWindowTextW(targets[i],title,256);
                std::wstring label=std::to_wstring(i+1)+L". "+(title[0]?title:L"ChatGPT")+L" (PID "+std::to_wstring(process_id(targets[i]))+L")";
                AppendMenuW(choices,MF_STRING|(targets[i]==selected?MF_CHECKED:0),TargetFirst+i,label.c_str());
            }
            AppendMenuW(menu,MF_POPUP,reinterpret_cast<UINT_PTR>(choices),L"选择 ChatGPT 窗口");
        }
        AppendMenuW(menu,MF_SEPARATOR,0,nullptr);
        AppendMenuW(menu,MF_STRING,OpenConfig,L"打开配置目录");
        AppendMenuW(menu,MF_STRING|(startup_enabled()?MF_CHECKED:0)|(test?MF_GRAYED:0),Startup,L"开机自启");
        AppendMenuW(menu,MF_STRING,Help,L"使用说明");
        AppendMenuW(menu,MF_SEPARATOR,0,nullptr);AppendMenuW(menu,MF_STRING,Quit,L"退出");
        POINT cursor{};GetCursorPos(&cursor);SetForegroundWindow(host);
        UINT command=TrackPopupMenu(menu,TPM_RETURNCMD|TPM_RIGHTBUTTON,cursor.x,cursor.y,0,host,nullptr);
        DestroyMenu(menu);PostMessageW(host,WM_NULL,0,0);
        auto value=data();Shell_NotifyIconW(NIM_SETFOCUS,&value);return command;
    }
    ~NativeShell(){
        if(hotkey)UnregisterHotKey(host,hotkeyId);
        if(added){auto value=data();Shell_NotifyIconW(NIM_DELETE,&value);}
        if(icon)DestroyIcon(icon);
    }
};
