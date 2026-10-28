//! Lexical remote paths. Never interpret a remote path using the host OS.
use crate::FileManagerError;
use pab_protocol::{NativeFilePath, PathStyle};

fn units(path: &NativeFilePath, style: PathStyle) -> Result<Vec<u16>, FileManagerError> {
    path.validate().map_err(FileManagerError::Invalid)?;
    match path {
        NativeFilePath::Utf8(value) => Ok(value.encode_utf16().collect()),
        NativeFilePath::WindowsWide(value) if style == PathStyle::Windows => Ok(value.clone()),
        NativeFilePath::UnixBytes(value) if style == PathStyle::Posix => Ok(value.iter().map(|v| *v as u16).collect()),
        _ => Err(FileManagerError::Invalid("path encoding does not match target platform")),
    }
}
fn from_units(template: &NativeFilePath, value: Vec<u16>) -> Result<NativeFilePath, FileManagerError> {
    let result = match template {
        NativeFilePath::Utf8(_) => NativeFilePath::Utf8(String::from_utf16(&value).map_err(|_| FileManagerError::Invalid("invalid UTF-16 filename"))?),
        NativeFilePath::WindowsWide(_) => NativeFilePath::WindowsWide(value),
        NativeFilePath::UnixBytes(_) => NativeFilePath::UnixBytes(value.into_iter().map(|v| u8::try_from(v).map_err(|_| FileManagerError::Invalid("invalid native byte filename"))).collect::<Result<_,_>>()?),
    };
    result.validate().map_err(FileManagerError::Invalid)?;
    Ok(result)
}
fn separator(value: u16, style: PathStyle) -> bool { value == 47 || style == PathStyle::Windows && value == 92 }
fn root_length(path: &[u16], style: PathStyle) -> Result<usize, FileManagerError> {
    let invalid = || FileManagerError::Invalid("expected an absolute native path");
    if style == PathStyle::Posix { return if path.first() == Some(&47) { Ok(1) } else { Err(invalid()) }; }
    let sep = |v| separator(v, style);
    let drive = |start: usize| path.len() >= start + 3 && path[start] < 128 && (path[start] as u8).is_ascii_alphabetic() && path[start+1] == 58 && sep(path[start+2]);
    if drive(0) { return Ok(3); }
    if path.len() < 2 || !sep(path[0]) || !sep(path[1]) { return Err(invalid()); }
    let mut start = 2;
    if path.get(2) == Some(&63) && path.get(3).is_some_and(|v| sep(*v)) {
        if drive(4) { return Ok(7); }
        if path.len() < 8 || !path[4..7].iter().copied().zip([85u16,78,67]).all(|(a,b)| a == b || a == b + 32) || !sep(path[7]) { return Err(invalid()); }
        start = 8;
    } else if path.get(2) == Some(&46) { return Err(invalid()); }
    // UNC roots include exactly the server and share; these cannot be renamed.
    for _ in 0..2 {
        let end = path[start..].iter().position(|v| sep(*v)).map(|n| start+n).unwrap_or(path.len());
        if end == start || path[start..end] == [46] || path[start..end] == [46,46] { return Err(invalid()); }
        start = if end < path.len() { end+1 } else { end };
    }
    Ok(start)
}
fn checked(path: &NativeFilePath, style: PathStyle) -> Result<(Vec<u16>, usize), FileManagerError> {
    let mut value = units(path, style)?;
    let root = root_length(&value, style)?;
    if value[root..].split(|v| separator(*v, style)).any(|part| part == [46] || part == [46,46]) {
        return Err(FileManagerError::Invalid("relative components are not allowed"));
    }
    while value.len() > root && value.last().is_some_and(|v| separator(*v,style)) { value.pop(); }
    Ok((value,root))
}
fn comparison_key(path:&NativeFilePath,style:PathStyle)->Result<Vec<u16>,FileManagerError> {
    let (mut value,_)=checked(path,style)?;
    if style==PathStyle::Posix {
        if let NativeFilePath::Utf8(text)=path {value=text.trim_end_matches('/').as_bytes().iter().map(|byte|*byte as u16).collect();}
        if value.is_empty(){value.push(47);}
    } else {
        for unit in &mut value {if *unit==47 {*unit=92;}if (97..=122).contains(unit){*unit-=32;}}
        if value.starts_with(&[92,92,63,92,85,78,67,92]) {value.splice(..8,[92,92]);}
        else if value.starts_with(&[92,92,63,92]) {value.drain(..4);}
    }
    // Repeated separators do not create distinct path components.
    let mut normalized=Vec::with_capacity(value.len());
    for unit in value {if unit==47 && style==PathStyle::Posix && normalized.last()==Some(&47){continue;}normalized.push(unit);}
    while normalized.len()>1 && normalized.last().is_some_and(|unit|separator(*unit,style)){normalized.pop();}
    Ok(normalized)
}
/// Preserve native bytes and the original spelling while removing duplicate
/// selections and descendants already covered by another selected directory.
pub fn independent_sources(paths:&[NativeFilePath],style:PathStyle)->Result<Vec<NativeFilePath>,FileManagerError> {
    let keys=paths.iter().map(|path|comparison_key(path,style)).collect::<Result<Vec<_>,_>>()?;
    let selected=keys.iter().cloned().collect::<std::collections::HashSet<_>>();
    let mut seen=std::collections::HashSet::new();let mut result=Vec::new();
    for (path,key) in paths.iter().zip(keys) {
        if !seen.insert(key){continue;}
        let mut parent=native_parent(path,style)?;let mut covered=false;
        while let Some(current)=parent {if selected.contains(&comparison_key(&current,style)?){covered=true;break;}parent=native_parent(&current,style)?;}
        if !covered {result.push(path.clone());}
    }
    Ok(result)
}
pub fn native_parent(path: &NativeFilePath, style: PathStyle) -> Result<Option<NativeFilePath>, FileManagerError> {
    let (value, root) = checked(path, style)?;
    if value.len() == root { return Ok(None); }
    let end = value.iter().rposition(|v| separator(*v,style)).unwrap().max(root);
    from_units(path, value[..end].to_vec()).map(Some)
}
pub fn native_name(path: &NativeFilePath, style: PathStyle) -> Result<NativeFilePath, FileManagerError> {
    let (value, root) = checked(path, style)?;
    if value.len() == root { return Err(FileManagerError::Invalid("filesystem root has no filename")); }
    let start = value.iter().rposition(|v| separator(*v,style)).map(|n| n+1).unwrap_or(root);
    from_units(path, value[start..].to_vec())
}
pub fn keep_both_candidate(path:&NativeFilePath,style:PathStyle,number:u16)->Result<NativeFilePath,FileManagerError> {
    if number==0 {return Err(FileManagerError::Invalid("copy suffix must be positive"));}
    let parent=native_parent(path,style)?.ok_or(FileManagerError::Invalid("cannot rename a root"))?;
    let name=native_name(path,style)?;
    let byte_utf8=matches!(name,NativeFilePath::Utf8(_)) && style==PathStyle::Posix;
    let value=if let NativeFilePath::Utf8(text)=&name && byte_utf8 {text.as_bytes().iter().map(|v|*v as u16).collect::<Vec<_>>()}else{units(&name,style)?};
    let dot=value.iter().rposition(|v|*v==46).filter(|n|*n>0 && value.len()-*n<=64).unwrap_or(value.len());
    let suffix=format!(" ({number})").encode_utf16().collect::<Vec<_>>();
    let mut end=dot.min(255-suffix.len()-(value.len()-dot));
    if byte_utf8 {while end>0 && value.get(end).is_some_and(|v|(*v as u8)&0xc0==0x80) {end-=1;}}
    else if end>0 && value.get(end-1).is_some_and(|v|(0xd800..=0xdbff).contains(v)) && value.get(end).is_some_and(|v|(0xdc00..=0xdfff).contains(v)) {end-=1;}
    let mut candidate=value[..end].to_vec();candidate.extend(suffix);candidate.extend_from_slice(&value[dot..]);
    let candidate=if byte_utf8 {NativeFilePath::Utf8(String::from_utf8(candidate.into_iter().map(|v|v as u8).collect()).map_err(|_|FileManagerError::Invalid("invalid filename"))?)}else{from_units(&name,candidate)?};
    native_join(&parent,&candidate,style)
}
pub fn native_join(parent: &NativeFilePath, name: &NativeFilePath, style: PathStyle) -> Result<NativeFilePath, FileManagerError> {
    name.validate_component().map_err(FileManagerError::Invalid)?;
    if style == PathStyle::Windows {
        let value = units(name,style)?;
        let bad = value.iter().any(|v| *v<32 || [47,92,60,62,58,34,124,63,42].contains(v)) || value.last().is_some_and(|v| *v==46 || *v==32);
        let stem = value.split(|v|*v==46).next().unwrap();
        let stem = String::from_utf16_lossy(stem).to_uppercase();
        let reserved = matches!(stem.as_str(),"CON"|"PRN"|"AUX"|"NUL"|"CONIN$"|"CONOUT$") || stem.strip_prefix("COM").or_else(||stem.strip_prefix("LPT")).is_some_and(|n|matches!(n,"1"|"2"|"3"|"4"|"5"|"6"|"7"|"8"|"9"|"¹"|"²"|"³"));
        if bad || reserved {return Err(FileManagerError::Invalid("filename is not representable on Windows"));}
    }
    // Promote to the lossless native representation when one part requires it.
    let template = match (parent,name) {
        (NativeFilePath::WindowsWide(_), _) | (_, NativeFilePath::WindowsWide(_)) if style == PathStyle::Windows => NativeFilePath::WindowsWide(vec![1]),
        (NativeFilePath::UnixBytes(_), _) | (_, NativeFilePath::UnixBytes(_)) if style == PathStyle::Posix => NativeFilePath::UnixBytes(vec![1]),
        _ => parent.clone(),
    };
    let convert = |path: &NativeFilePath| -> Result<Vec<u16>,FileManagerError> {
        if matches!(template,NativeFilePath::UnixBytes(_)) {
            return match path { NativeFilePath::Utf8(s) => Ok(s.as_bytes().iter().map(|v| *v as u16).collect()), _ => units(path,style) };
        }
        units(path,style)
    };
    checked(parent,style)?;
    let mut value = convert(parent)?;
    while value.last().is_some_and(|v| separator(*v,style)) { value.pop(); }
    value.push(if style == PathStyle::Windows { 92 } else { 47 });
    value.extend(convert(name)?);
    let result = from_units(&template,value)?;
    checked(&result,style)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_selections_deduplicate_native_encodings_and_parent_child_order() {
        let raw=NativeFilePath::UnixBytes(b"/a/child".to_vec());
        assert_eq!(independent_sources(&[raw,"/a".into(),"/a/".into(),"/b".into()],PathStyle::Posix).unwrap(),vec![NativeFilePath::from("/a"),NativeFilePath::from("/b")]);
        let wide=NativeFilePath::WindowsWide("C:\\DATA\\child".encode_utf16().collect());
        assert_eq!(independent_sources(&[wide,"c:/data".into(),"\\\\?\\C:\\data\\".into(),"D:\\other".into()],PathStyle::Windows).unwrap(),vec![NativeFilePath::from("c:/data"),NativeFilePath::from("D:\\other")]);
        assert_eq!(independent_sources(&["/aa".into(),"/a".into()],PathStyle::Posix).unwrap().len(),2);
        assert_eq!(independent_sources(&[NativeFilePath::UnixBytes(vec![47,255]),NativeFilePath::UnixBytes(vec![47,254])],PathStyle::Posix).unwrap().len(),2);
    }
    #[test]
    fn a_unicode_backslash_component_is_native_on_posix_and_invalid_on_windows() {
        let name:NativeFilePath="中文\\file".into();
        assert_eq!(native_join(&"/tmp".into(),&name,PathStyle::Posix).unwrap(),NativeFilePath::from("/tmp/中文\\file"));
        assert!(native_join(&"C:\\tmp".into(),&name,PathStyle::Windows).is_err());
    }
    #[test]
    fn remote_paths_preserve_roots_and_native_components_on_any_host() {
        for root in ["C:\\", "\\\\server\\share", "\\\\server\\share\\", "\\\\?\\C:\\", "\\\\?\\UNC\\server\\share\\"] {
            let root: NativeFilePath = root.into();
            assert_eq!(native_parent(&root,PathStyle::Windows).unwrap(),None);
            let child = native_join(&root,&"目录".into(),PathStyle::Windows).unwrap();
            assert_eq!(native_name(&child,PathStyle::Windows).unwrap(),"目录".into());
            assert!(native_parent(&child,PathStyle::Windows).unwrap().is_some());
        }
        assert_eq!(native_parent(&"/".into(),PathStyle::Posix).unwrap(),None);
        assert_eq!(native_parent(&"/a".into(),PathStyle::Posix).unwrap(),Some("/".into()));
        let raw = NativeFilePath::UnixBytes(vec![255,b'\\']);
        let joined = native_join(&"/中文".into(),&raw,PathStyle::Posix).unwrap();
        assert_eq!(native_name(&joined,PathStyle::Posix).unwrap(),raw);
        let raw = NativeFilePath::WindowsWide(vec![0xd800]);
        let joined = native_join(&"C:\\中文".into(),&raw,PathStyle::Windows).unwrap();
        assert_eq!(native_name(&joined,PathStyle::Windows).unwrap(),raw);
        for path in ["C:relative", "\\rooted", "\\\\.\\C:\\", "C:\\a\\..\\b"] { assert!(native_parent(&path.into(),PathStyle::Windows).is_err()); }
        assert!(native_join(&"/".into(),&"../escape".into(),PathStyle::Posix).is_err());
        for name in ["file:stream","CON.txt","LPT1","trailing.","bad?"] {assert!(native_join(&"C:\\".into(),&name.into(),PathStyle::Windows).is_err());}
        assert_eq!(keep_both_candidate(&"C:\\dir\\file.txt".into(),PathStyle::Windows,2).unwrap(),"C:\\dir\\file (2).txt".into());
        assert_eq!(keep_both_candidate(&"/dir/.hidden".into(),PathStyle::Posix,1).unwrap(),"/dir/.hidden (1)".into());
        let long:NativeFilePath=format!("/dir/{}.txt","中".repeat(80)).into();
        let NativeFilePath::Utf8(short)=native_name(&keep_both_candidate(&long,PathStyle::Posix,9).unwrap(),PathStyle::Posix).unwrap() else {panic!()};
        assert!(short.len()<=255);assert!(short.ends_with(" (9).txt"));
    }
}
