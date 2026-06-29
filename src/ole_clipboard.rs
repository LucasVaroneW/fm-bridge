// Native IDataObject impl for FileMaker's OLE clipboard on Windows.
//
// FileMaker's Ctrl+V only fires when clipboard data comes from a "proper"
// IDataObject via OleSetClipboard — not from raw SetClipboardData, and not
// from SHCreateDataObject's stock IDataObject. .NET's System.Windows.Forms
// .DataObject works, which is what this module mirrors: a minimal IDataObject
// holding a single (format_id, bytes) pair, plus a working EnumFormatEtc.

#![cfg(windows)]

use windows::Win32::Foundation::{
    BOOL, DV_E_FORMATETC, DV_E_TYMED, E_NOTIMPL, OLE_E_ADVISENOTSUPPORTED, S_OK,
};
use windows::Win32::System::Com::{
    DVASPECT_CONTENT, FORMATETC, IAdviseSink, IDataObject, IDataObject_Impl, IEnumFORMATETC,
    IEnumSTATDATA, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::UI::Shell::SHCreateStdEnumFmtEtc;
use windows::core::{HRESULT, Result as WResult, implement};

const DATADIR_GET: u32 = 1;

#[implement(IDataObject)]
pub struct FmDataObject {
    format_id: u16,
    data: Vec<u8>,
}

impl FmDataObject {
    pub fn new(format_id: u16, data: Vec<u8>) -> Self {
        Self { format_id, data }
    }
}

impl IDataObject_Impl for FmDataObject_Impl {
    fn GetData(&self, pformatetcin: *const FORMATETC) -> WResult<STGMEDIUM> {
        unsafe {
            let fmt = &*pformatetcin;
            if fmt.cfFormat != self.format_id {
                return Err(DV_E_FORMATETC.into());
            }
            if (fmt.tymed & TYMED_HGLOBAL.0 as u32) == 0 {
                return Err(DV_E_TYMED.into());
            }
            let hglobal = GlobalAlloc(GMEM_MOVEABLE, self.data.len())?;
            let ptr = GlobalLock(hglobal) as *mut u8;
            if ptr.is_null() {
                return Err(windows::core::Error::from_win32());
            }
            std::ptr::copy_nonoverlapping(self.data.as_ptr(), ptr, self.data.len());
            let _ = GlobalUnlock(hglobal);
            Ok(STGMEDIUM {
                tymed: TYMED_HGLOBAL.0 as u32,
                u: STGMEDIUM_0 { hGlobal: hglobal },
                pUnkForRelease: std::mem::ManuallyDrop::new(None),
            })
        }
    }

    fn GetDataHere(&self, _: *const FORMATETC, _: *mut STGMEDIUM) -> WResult<()> {
        Err(E_NOTIMPL.into())
    }

    fn QueryGetData(&self, pformatetc: *const FORMATETC) -> HRESULT {
        unsafe {
            let fmt = &*pformatetc;
            if fmt.cfFormat == self.format_id && (fmt.tymed & TYMED_HGLOBAL.0 as u32) != 0 {
                S_OK
            } else {
                DV_E_FORMATETC
            }
        }
    }

    fn GetCanonicalFormatEtc(&self, _: *const FORMATETC, pformatetcout: *mut FORMATETC) -> HRESULT {
        unsafe {
            (*pformatetcout).ptd = std::ptr::null_mut();
        }
        DV_E_FORMATETC
    }

    fn SetData(&self, _: *const FORMATETC, _: *const STGMEDIUM, _: BOOL) -> WResult<()> {
        Err(E_NOTIMPL.into())
    }

    fn EnumFormatEtc(&self, dwdirection: u32) -> WResult<IEnumFORMATETC> {
        if dwdirection != DATADIR_GET {
            return Err(E_NOTIMPL.into());
        }
        let fmts = [FORMATETC {
            cfFormat: self.format_id,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0 as u32,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        }];
        unsafe { SHCreateStdEnumFmtEtc(&fmts) }
    }

    fn DAdvise(&self, _: *const FORMATETC, _: u32, _: Option<&IAdviseSink>) -> WResult<u32> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn DUnadvise(&self, _: u32) -> WResult<()> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn EnumDAdvise(&self) -> WResult<IEnumSTATDATA> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }
}
