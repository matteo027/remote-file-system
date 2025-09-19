import { Entity, Column, PrimaryColumn, ManyToOne, JoinColumn } from "typeorm";
import { File } from "./File";

@Entity()
export class Path {
    
  @PrimaryColumn({nullable:false, unique:true})
  path:string;

  @ManyToOne(() => File, (file) => file.paths, { nullable: false })
  @JoinColumn({ name: "ino", referencedColumnName: "ino" })
  file: File;

}