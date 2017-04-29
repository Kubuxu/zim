//! This crate provides a pure-rust library for reading ZIM files.
//!
//! ZIM files are a format used primarily to store wikis (such as Wikipedia and others based on
//! MediaWiki).
//!
//! For more into, see the [OpenZIM website](http://www.openzim.org/wiki/OpenZIM)
//! 

extern crate byteorder;
extern crate memmap;
extern crate xz_decom;

use byteorder::{LittleEndian, ReadBytesExt};
use std::io::Cursor;
use memmap::{Mmap, MmapView};
use xz_decom::{decompress, XZError};

use std::fs::File;
use std::io::Read;
use std::io::BufRead;
use std::path::Path;
use std::error::Error;
use std::convert::From;


/// An error type for parsing errors
pub struct ParsingError {
    msg: &'static str,
    cause: Option<Box<Error>>
}

impl From<XZError> for ParsingError {
    fn from(e: XZError) -> ParsingError {
        ParsingError {
            msg: "Error decoding compressed data",
            cause: Some(Box::new(e))
        }
    }
}

impl From<byteorder::Error> for ParsingError {
    fn from(e: byteorder::Error) -> ParsingError {
        ParsingError {
            msg: "Error reading bytestream",
            cause: Some(Box::new(e))
        }
    }
}

impl From<std::string::FromUtf8Error> for ParsingError {
    fn from(e: std::string::FromUtf8Error) -> ParsingError {
        ParsingError {
            msg: "Error converting to string",
            cause: Some(Box::new(e))
        }
    }
}

impl From<std::io::Error> for ParsingError {
    fn from(e: std::io::Error) -> ParsingError {
        ParsingError {
            msg: "Error reading bytestream",
            cause: Some(Box::new(e))
        }
    }
}


#[derive(Debug, PartialEq)]
pub enum MimeType {
    /// A special "MimeType" that represents a redirection
    Redirect,
    LinkTarget,
    DeletedEntry,
    Type(String)
}

#[derive(Debug, PartialEq)]
pub enum Target {
    /// Redirect specified as a URL index
    Redirect(u32),
    /// Cluster index and blob index
    Cluster(u32, u32)
}

/// A cluster of blobs
///
/// Within an ZIM archive, clusters contain several blobs of data that are all compressed together.
/// Each blob is the data for an article.
//#[derive(Debug)]
pub struct Cluster {
    start_off: u64,
    end_off: u64,
    comp_type: u8,
    blob_list: Vec<u32>, // offsets into data
    data: Vec<u8>,
    
}

impl Cluster {
    fn new(zim: &Zim, idx: u32) -> Result<Cluster, ParsingError> {
        let idx = idx as usize;
        let this_cluster_off = zim.cluster_list[idx];
        let next_cluster_off = if idx < zim.cluster_list.len()-1 {
            zim.cluster_list[idx + 1]
        } else {
            zim.checksum_off
        };

        assert!(next_cluster_off > this_cluster_off);
        let total_cluster_size: usize = (next_cluster_off - this_cluster_off) as usize;

        let cluster_view = {
            let mut view = unsafe{ zim.master_view.clone() };
            let len = view.len();
            view.restrict(this_cluster_off as usize, total_cluster_size);
            view
        };
        let slice = unsafe{ cluster_view.as_slice() };
        let comp_type = slice[0];
        let mut blob_list = Vec::new(); 
        let data: Vec<u8> = if comp_type == 4 {
            let data = try!(decompress(&slice[1..total_cluster_size]));
            println!("Decompressed {} bytes of data", data.len());
            data
        } else {
            Vec::from(&slice[1..total_cluster_size])
        };
        let datalen = data.len();
        {
            let mut cur = Cursor::new(&data);
            loop {
                let offset = try!(cur.read_u32::<LittleEndian>());
                blob_list.push(offset);
                if offset as usize >= datalen {
                    //println!("at end");
                    break;
                }
            }
        }

        Ok(Cluster {
            comp_type: comp_type,
            start_off: this_cluster_off,
            end_off: next_cluster_off,
            data: data,
            blob_list: blob_list,
        })
        
    }
    pub fn get_blob(&self, idx: u32) -> &[u8] {
        let this_blob_off = self.blob_list[idx as usize] as usize;
	if self.blob_list.len() > idx as usize + 1 {
        let next_blob_off = self.blob_list[idx as usize + 1] as usize;
            &self.data[this_blob_off..next_blob_off]
        } else {
            &self.data[this_blob_off..]
        }
    }
}

/// Holds metadata about an article
#[derive(Debug)]
pub struct DirectoryEntry {
    pub mime_type: MimeType,
    pub namespace: char,
    pub revision: u32,
    pub url: String,
    pub title: String,
    pub target: Option<Target>
}

impl DirectoryEntry {
    fn new(zim: &Zim, s: &[u8]) -> Result<DirectoryEntry, ParsingError> {
        let mut cur = Cursor::new(s);
        let mime_id = try!(cur.read_u16::<LittleEndian>());
        let mime_type = try!(zim.get_mimetype(mime_id).ok_or(ParsingError{msg: "No such Mimetype", cause: None}));
        let _ = try!(cur.read_u8());
        let namespace = try!(cur.read_u8());
        let rev = try!(cur.read_u32::<LittleEndian>());
        let mut target = None;


        if mime_type == MimeType::Redirect {
            // this is an index into the URL table
            target = Some(Target::Redirect(try!(cur.read_u32::<LittleEndian>())));
        } else if mime_type == MimeType::LinkTarget || mime_type == MimeType::DeletedEntry {

        } else {
            let cluster_number = try!(cur.read_u32::<LittleEndian>());
            let blob_number = try!(cur.read_u32::<LittleEndian>());
            target = Some(Target::Cluster(cluster_number, blob_number));
        }
       
        let url = {
            let mut vec = Vec::new();
            let size = try!(cur.read_until(0, &mut vec));
            vec.truncate(size - 1);
            try!(String::from_utf8(vec))
        };
        let title = {
            let mut vec = Vec::new();
            let size = try!(cur.read_until(0, &mut vec));
            vec.truncate(size - 1);
            try!(String::from_utf8(vec))
        };


        Ok(DirectoryEntry{
            mime_type: mime_type,
            namespace: std::char::from_u32(namespace as u32).unwrap(),
            revision: rev,
            url: url,
            title: title,
            target: target,
        })
    }
}

/// Represents a ZIM file
#[allow(dead_code)]
pub struct Zim {
    // Zim structure data:

    version: u32,
    // uuid_1
    // uuid_2
    /// Number of articles in this archive
    pub article_count: u32,
    /// Number of clusters in this archive
    pub cluster_count: u32,
    url_tbl_off: u64, //offset from the start of the file
    title_tbl_off: u64, //offset from the start of the file
    cluster_tbl_off: u64,
    mime_tbl_off: u64, // should always be 80
    /// If Main Page is defined, this is the index to the page
    pub main_page_idx: Option<u32>, // an index into the url table
    layout_page_idx: Option<u32>,
    checksum_off: u64,

    // internal variables:
    f: File,
    master_view: MmapView,

    /// List of mimetypes used in this ZIM archive
    mime_table: Vec<String>, // a list of mimetypes
    url_list: Vec<u64>, // a list of offsets
    article_list: Vec<u32>, // a list of indicies into url_list
    cluster_list: Vec<u64>, // a list of offsets



}

pub struct DirectoryIterator<'a> {
    max_articles: u32,
    article_to_yield: u32,
    zim: &'a Zim
}

impl<'a> DirectoryIterator<'a> {
    fn new(zim: &'a Zim) -> DirectoryIterator<'a> {
        DirectoryIterator {
            max_articles: zim.article_count,
            article_to_yield: 0,
            zim: zim
        }
    }
}

impl<'a> std::iter::Iterator for DirectoryIterator<'a> {
    type Item = DirectoryEntry;
    fn next(&mut self) -> Option<Self::Item> {
        if self.article_to_yield >= self.max_articles {
            None 
        } else {
            let dir_entry_ptr = self.zim.url_list[self.article_to_yield as usize] as usize;
            self.article_to_yield += 1;
            let dir_view = {
                let mut view = unsafe{ self.zim.master_view.clone() };
                let len = view.len();
                view.restrict(dir_entry_ptr, len - dir_entry_ptr);
                view
            };
            let slice = unsafe{ dir_view.as_slice() };

            if let Ok(entry) = DirectoryEntry::new(self.zim, slice) {
                Some(entry)
            } else {
                None
            }
        }
    }
}

impl Zim {
    /// Loads a Zim file
    ///
    /// Loads a Zim file and parses the header, and the url, title, and cluster offset tables.  The
    /// rest of the data isn't parsed until it's needed, so this should be fairly quick.
    pub fn new<P: AsRef<Path>>(p: P) -> Result<Zim, ParsingError> {
        let mut f = try!(File::open(p));
        let mmap = try!(Mmap::open(&f, memmap::Protection::Read));
        let master_view = mmap.into_view();

        let header_view = {
            let mut view = unsafe{ master_view.clone() };
            view
        };

        let mut header_cur = Cursor::new( unsafe{ header_view.as_slice() } );

        let magic = try!(header_cur.read_u32::<LittleEndian>());
        assert_eq!(magic, 72173914);
        let version = try!(header_cur.read_u32::<LittleEndian>());
        let uuid_1 = try!(header_cur.read_u64::<LittleEndian>());
        let uuid_2 = try!(header_cur.read_u64::<LittleEndian>());
        let article_count = try!(header_cur.read_u32::<LittleEndian>());
        let cluster_count = try!(header_cur.read_u32::<LittleEndian>());
        let url_ptr_pos = try!(header_cur.read_u64::<LittleEndian>());
        let title_ptr_pos = try!(header_cur.read_u64::<LittleEndian>());
        let cluster_ptr_pos = try!(header_cur.read_u64::<LittleEndian>());
        let mime_list_pos = try!(header_cur.read_u64::<LittleEndian>());
        assert_eq!(mime_list_pos, 80);
        let main_page = try!(header_cur.read_u32::<LittleEndian>());
        let layout_page = try!(header_cur.read_u32::<LittleEndian>());
        let checksum_pos = try!(header_cur.read_u64::<LittleEndian>());
        assert_eq!(header_cur.position(), 80);

        println!("version: {}", version);
        println!("article_count: {}", article_count);
        println!("cluster_count: {}", cluster_count);
        println!("mime_list_pos: {}", mime_list_pos);


        // the mime table is always directly after the 80-byte header, so we'll keep
        // using our header cursor 
        let mime_table = {
            let mut mime_table = Vec::new();
            loop {
                let mut mime_buf = Vec::new();
                if let Ok(size) = header_cur.read_until(0, &mut mime_buf) {
                    if size <= 1 { break; }
                    mime_buf.truncate(size - 1);
                    mime_table.push(try!(String::from_utf8(mime_buf)));
                }
            }
            mime_table
        };

        let url_list = {
            let mut list = Vec::new();
            let url_list_view = { let mut v = unsafe{master_view.clone()};
                v.restrict(url_ptr_pos as usize, article_count as usize * 8);
                v };
            let mut url_cur = Cursor::new( unsafe{ url_list_view.as_slice() });

            for url_num in 0..article_count {
                let pointer = try!(url_cur.read_u64::<LittleEndian>());
                list.push(pointer);
            }
            list
        };
        
        let article_list = {
            let mut list = Vec::new();
            let art_list_view = { let mut v = unsafe{master_view.clone()};
                v.restrict(title_ptr_pos as usize, article_count as usize * 8);
                v };
            let mut art_cur = Cursor::new( unsafe{ art_list_view.as_slice() });

            for _ in 0..article_count {
                let url_number = try!(art_cur.read_u32::<LittleEndian>());
                list.push(url_number);
            }
            list
        };


        let cluster_list = {
            let mut list = Vec::new();
            let cluster_list_view = { let mut v = unsafe{master_view.clone()};
                v.restrict(cluster_ptr_pos as usize, cluster_count as usize * 8);
                v };
            let mut cluster_cur = Cursor::new( unsafe{ cluster_list_view.as_slice() });

            for cluster_num in 0..cluster_count {
                let pointer = try!(cluster_cur.read_u64::<LittleEndian>());
                list.push(pointer);
            }
            list
        };


        
        Ok(Zim {
           version: version,
           article_count: article_count,
           cluster_count: cluster_count,
           url_tbl_off: url_ptr_pos,
           title_tbl_off: title_ptr_pos,
           cluster_tbl_off: cluster_ptr_pos,
           mime_tbl_off: mime_list_pos,
           main_page_idx: if main_page ==  0xffffffff { None } else { Some(main_page) },
           layout_page_idx: if layout_page == 0xffffffffff { None } else { Some(layout_page) },
           checksum_off: checksum_pos,

           f: f,
           master_view: master_view,
           mime_table: mime_table,
           url_list: url_list,
           article_list: article_list,
           cluster_list: cluster_list,

        })

    }

    /// Indexes into the ZIM mime_table.  
    pub fn get_mimetype(&self, id: u16) -> Option<MimeType> {
        match id {
            0xffff => Some(MimeType::Redirect),
            0xfffe => Some(MimeType::LinkTarget),
            0xfffd => Some(MimeType::DeletedEntry),
            id => {
                if (id as usize) < self.mime_table.len() {
                     Some(MimeType::Type(self.mime_table[id as usize].clone()))
                } else {
                    println!("WARNINING unknown mimetype idx {}", id);
                    None
                }
            }
        }
    }

    /// Iterates over articles, sorted by URL.
    ///
    /// For performance reasons, you might want to extract by cluster instead.
    pub fn iterate_by_urls(&self) -> DirectoryIterator {
        DirectoryIterator::new(self)     
    }

    /// Returns the `DirectoryEntry` for the article found at the given URL index.
    ///
    /// idx must be between 0 and `article_count`
    pub fn get_by_url_index(&self, idx: u32) -> Option<DirectoryEntry> {
        let entry_offset = self.url_list[idx as usize] as usize;
        let dir_view = {
            let mut view = unsafe{ self.master_view.clone() };
            let len = view.len();
            view.restrict(entry_offset, len - entry_offset);
            view
        };
        let slice = unsafe{ dir_view.as_slice() };
        DirectoryEntry::new(self, slice).ok()
    }

    /// Returns the given `Cluster`
    /// 
    /// idx must be between 0 and `cluster_count`
    pub fn get_cluster(&self, idx: u32) -> Option<Cluster> {
        Cluster::new(self, idx).ok()
    }

}



#[test]
fn test_zim() {

    // we want to handle all URLs from the same cluster at the same time,
    // so build a map between cluster
    // build a mapping from 

    //println!("{:?}", zim.get_by_url_index(59357));

    //let cluster = zim.get_cluster(201);
    //let data = cluster.get_blob(6);
    //let s = std::str::from_utf8(data).unwrap();
    //println!("Cluster: {:?}", cluster);
    //println!("data: {}", s);


}
